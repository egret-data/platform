# Cấp phát StarRocks — Operator + `StarRocksCluster` CR (shared-data) — Thiết kế

**Ngày:** 2026-07-16 · **Trạng thái:** Thiết kế — chờ duyệt · **Nhánh:** `feat/starrocks-provisioning` (off main)
**Chương trình:** StarRocks ngang-hàng-với-Trino, **lát 2/3**. Lát 1 (cưỡng chế Ranger) đã ship (#259) + follow-up (#260). Lát 3 (delegation/bảo mật) là spec sau. Lát này: **cấp phát cụm StarRocks lên Kubernetes từ control plane**.

> Bản chuẩn tiếng Anh: `docs/en/specs/2026-07-16-starrocks-provisioning-design.md`. Đây là bản tham chiếu.

## Mục tiêu

Cho platform admin cấp phát/quản lý/xoá engine **StarRocks** từ control plane, ngang Trino — cụm được cấp phát **tự-nối embedded Iceberg catalog**, **cưỡng chế Ranger** (lát 1, giờ do provisioner template thay vì deploy thủ công), và **gán được vào workspace**. Chứng minh live bằng một truy vấn thật qua cụm đã cấp phát.

**Topology:** FE + CN ở chế độ **shared-data** của StarRocks — hợp lakehouse của egret (dữ liệu ở object store, CN stateless). Đã chọn cùng người dùng (thay vì FE+BE local PV).

## Ngoài phạm vi

- **egret KHÔNG quản lý vòng đời operator.** Operator `kube-starrocks` + CRD được **cài sẵn một lần** (tài liệu trong `deploy/`), y như Stackable Trino operator cài sẵn. Provisioner giả định CRD `StarRocksCluster` đã có.
- **MVP:** 1 FE (không HA) + 1 CN. HA-FE, autoscale CN, config surface đầy đủ = follow-up.
- **BE / shared-nothing: không** (chỉ shared-data lát này). FE+BE có thể thêm sau sau cùng provisioner.
- **Không thêm authz.** Cưỡng chế Ranger là lát 1; lát này chỉ dời config FE của nó từ `deploy/` vào provisioner.

## Bối cảnh (đã kiểm chứng)

- **Cấp phát Trino template raw k8s** (Deployment/ConfigMap/Secret/Service/Ingress mỗi role), KHÔNG dùng operator CR — engine `rgr2` đang chạy là Deployment thường (`rgr2-coordinator`/`-worker`), không phải `trinocluster` CR. StarRocks **cố tình khác**: dùng operator + CR (vì operator có sẵn local, và StarRocks — FE leader election, CN register, storage volume shared-data — khó tự-cuộn thành raw Deployments).
- **`KubeClient` đã apply CR sẵn**: `apply(obj: &Value)` suy `ApiResource` từ `apiVersion`+`kind`, server-side apply dạng `DynamicObject`. ⇒ apply `StarRocksCluster` CR + ConfigMap KHÔNG cần đổi KubeClient. (kube-rs 0.99.)
- **`EngineProvisioner`** (provision/status/update/delete); `TrinoProvisioner` dựng 1 lần ở `main.rs:1300`; `create_engine(name, "trino", …)` (`engines.rs:1634`) đóng cứng `"trino"` — cần **registry provisioner theo type** (giờ 2 loại engine).
- **`RangerBinding`** (`egret-engine-mgmt-api`) đã mô hình hoá wiring Ranger; lát 1 giao config FE ở `deploy/starrocks/ranger/`. Lát này dời vào ConfigMap FE của provisioner.
- Gán engine→workspace (#247/#248) bất-khả-tri-engine ⇒ tái dùng.
- Operator repo checkout ở `data-portal/starrocks-kubernetes-operator/`: CRD `deploy/starrocks.com_starrocksclusters.yaml`, cài `deploy/operator.yaml`, ví dụ `examples/starrocks/`.

## Hình dạng CR `StarRocksCluster`

`apiVersion: starrocks.com/v1`, `kind: StarRocksCluster`. `spec`:
- `starRocksFeSpec`: `image` (`starrocks/fe-ubuntu`), `replicas`, `limits`/`requests`, `storageVolumes`, `configMapInfo: {configMapName, resolveKey: fe.conf}` (nạp config FE), và **`configMaps: [{name, mountPath}]`** (mount thêm ConfigMap — XML Ranger nằm ở đây, mount vào thư mục conf của FE).
- `starRocksCnSpec`: `image` (`starrocks/cn-ubuntu`), `replicas`, `limits`/`requests` (stateless).

**`fe.conf` shared-data** (trong ConfigMap FE): ports (`query_port=9030`…), `run_mode = shared_data`, `cloud_native_meta_port = 6090`, `enable_load_volume_from_conf = true` + tham số object-store để tạo **builtin storage volume** lúc FE bootstrap:
```
cloud_native_storage_type = S3
aws_s3_endpoint = http://host.docker.internal:9000
aws_s3_path = <bucket>/<prefix>
aws_s3_access_key = <key>; aws_s3_secret_key = <secret>
aws_s3_use_aws_sdk_default_behavior = false
aws_s3_enable_path_style_access = true
```
…cộng `access_control = ranger` khi engine bind vào policy profile.

## Kiến trúc

1. **`StarRocksProvisioner`** (`egret-orchestrator/src/starrocks.rs`, impl `EngineProvisioner`): provision = apply ConfigMap FE (`fe.conf`) + ConfigMap Ranger XML (khi `spec.ranger`) + CR `StarRocksCluster` (qua `KubeClient::apply`); status = từ pod FE+CN; update = re-apply (idempotent); delete = xoá CR + ConfigMap. `StarRocksConfig` (giống `TrinoConfig`) + các hàm template thuần (`starrocks_cluster_cr`/`starrocks_fe_configmap`/`starrocks_ranger_configmap`) có unit test trên JSON.
2. **Registry provisioner** — `main.rs` dựng map theo type `{trino, starrocks}` sau `Arc<dyn EngineProvisioner>`; dispatch theo `engine.type`.
3. **Config surface** (`egret-engine-mgmt-api`) — `StarRocksEngineConfig` (fe/cn image, replicas, resources; storage mặc định từ `config.storage`).
4. **Auto-wire Iceberg** (runtime): sau khi FE Ready, chạy `CREATE EXTERNAL CATALOG … iceberg type=rest uri=<engine_egret_url>/catalog` qua FE mysql (tái dùng `engine_egret_url` + token catalog như Trino).
5. **Ranger auto-wire**: `fe.conf` `access_control=ranger` + mount XML, bind theo `spec.ranger` (`RangerBinding`) — config lát 1, giờ do provisioner template.
6. **Gán workspace**: không đổi (#247/#248).

## Luồng dữ liệu

Admin tạo engine StarRocks (bind egret warehouse) → `create_engine(…, "starrocks", …)` mint token catalog + dựng `EngineSpec` → provisioner apply ConfigMap FE/Ranger + CR → operator tạo pod FE+CN → FE bootstrap builtin volume MinIO + (nếu bind) nạp policy Ranger từ egret → auto-wire `CREATE EXTERNAL CATALOG` → engine truy vấn được (đọc Iceberg external) + cưỡng chế Ranger nếu bind.

## Kiểm thử

- **Unit (CI):** các hàm template phát đúng JSON (FE shared-data + storage, CN, mounts, `access_control=ranger` chỉ khi có `spec.ranger`); registry route `"starrocks"` đúng; fake `KubeClient`.
- **DB/HTTP (CI):** `create_engine("starrocks")` lưu engine + catalog row.
- **CHỈ-LIVE (controller):** cài operator → cấp phát engine qua control plane → pod FE+CN Ready → tạo external catalog → truy vấn thật trả rows từ bảng Iceberg seeded; nếu bind profile, truy vấn masked/denied xác nhận Ranger cưỡng chế trên cụm ĐÃ CẤP PHÁT. Báo cáo đúng những gì đã chạy.

## Rủi ro chỉ-live

1. Bootstrap storage shared-data (builtin volume trên MinIO, path-style, host.docker.internal:9000); fallback `CREATE STORAGE VOLUME` runtime.
2. Đường mount XML Ranger vào classpath/conf dir của image `fe-ubuntu` (lát 1 thấy `/data/deploy/starrocks/fe/conf` trên allin1 — xác nhận trên fe-ubuntu).
3. Ảnh operator/fe/cn có sẵn trong kind (pre-pull nếu offline).
4. Map CR ↔ pod status cho UI sức khoẻ control-plane.

## Phân rã (cho plan)

1. Cài operator (`deploy/starrocks-operator/` + README). 2. `StarRocksProvisioner` + template fns + `StarRocksConfig` (unit test). 3. Registry provisioner theo type. 4. Config surface + `create_engine("starrocks")`. 5. Auto-wire Iceberg + Ranger từ `spec.ranger`. 6. FE control-plane (form config StarRocks). 7. Cổng CI + LIVE SMOKE cấp-phát-rồi-truy-vấn.
