# StarRocks — Cấu hình Cluster Chi tiết (ngang Trino) — Thiết kế

**Ngày:** 2026-07-17 · **Trạng thái:** Thiết kế — chờ duyệt · **Nhánh:** `feat/starrocks-deep-config` (off `main`)
**Chương trình:** StarRocks-ngang-Trino, follow-on về độ-sâu-config. Epic trước (#265) cho StarRocks tab Configuration/Security ở mức *cốt lõi* (image/replicas/resources/delegation). Phần này đưa **độ sâu** cấu hình cluster ngang trang engine-detail của Trino — override `fe.conf`/`cn.conf`, autoscaling CN, và Pod & Containers/scheduling.

> Bản chuẩn tiếng Anh: `docs/en/specs/2026-07-17-starrocks-deep-config-design.md`.

## Vấn đề

Tab Configuration của Trino cho tune sâu (Server: query max memory, log level, config props tuỳ ý, JVM/env; Workload; Pod & Containers: serviceAccount, security context, nodeSelector, affinity, tolerations, env, init/sidecar; Autoscaling HPA/KEDA). StarRocks hiện chỉ có Image / Compute / Resources / Storage. `fe.conf` egret sinh **cứng** (`starrocks_fe_configmap` — `JAVA_OPTS -Xmx8192m`, `sys_log_level = INFO`, ports, run_mode) không cho admin override; **không có** `cn.conf`; CR không mang autoscaling hay pod/scheduling — dù operator `StarRocksCluster` CR hỗ trợ đầy đủ (`autoScalingPolicy` trên CN; LoadSpec `serviceAccount`/`nodeSelector`/`affinity`/`tolerations`/`annotations`/`initContainers`/`sidecars`/`imagePullSecrets`).

## Mục tiêu

Đào sâu tab Configuration StarRocks tới mức Trino bằng 4 section, đều sửa-được + save-and-reprovision (tái dùng máy update/restart của #265):

1. **FE config** — override key-value `fe.conf` (JVM heap qua `JAVA_OPTS`, `sys_log_level`, query timeout, key tuỳ ý), merge đè lên base egret sinh (admin thắng).
2. **CN config** — override `cn.conf` qua ConfigMap CN mới, gắn vào `starRocksCnSpec.configMapInfo`.
3. **Autoscaling** — CN HPA (`autoScalingPolicy.hpaPolicy`): bật + min/max replicas + target CPU%.
4. **Pod & Containers** — serviceAccount, nodeSelector, annotations, env, tolerations/affinity/securityContext (raw-JSON, validate), init/sidecar, imagePullSecrets — áp cho **cả** FE và CN.

Chứng minh live: đặt override `fe.conf` + CN HPA + nodeSelector → Save re-provision → FE ConfigMap mang key merge, CN spec mang `autoScalingPolicy`, pod mang scheduling.

## Ngoài phạm vi

- **Tách Pod & Containers theo role** — MVP áp đều cả FE+CN; tách FE-riêng/CN-riêng để sau.
- **Topology BE (non-shared-data)** — chỉ shared-data FE+CN như hiện tại.
- **KEDA** — chỉ HPA.
- **Đổi storage/run_mode/key egret quản** — merge FE-config KHÔNG cho override key storage/run_mode/ssl/ranger/ports (egret giữ quyền); editor chỉ nhắm key *tuning*.
- **Cơ chế authz/delegation mới** — không đổi so với #265.

## Bối cảnh (đã kiểm chứng)

- `starrocks_fe_configmap` (`orchestrator/starrocks.rs:40`) dựng `fe.conf` bằng `push_str`, trả `json!({…,"data":{"fe.conf":conf}})`. Không có ConfigMap CN.
- `starrocks_cluster_cr` (`:211`) set image/replicas/limits/requests/configMapInfo(FE) + SSL keystore + restart annotation. Không autoscaling, không pod/scheduling.
- `StarRocksConfig` (`engine-mgmt-api/lib.rs:407`) là contract dựng CR; `build_starrocks_provision_req` ráp từ `engine.spec["starrocksConfig"]`, `build_starrocks_spec` dựng từ body (đều `engine-mgmt/engines.rs`, từ #265).
- Operator: `AutoScalingPolicy{version,hpaPolicy:{metrics,behavior},minReplicas,maxReplicas}`; LoadSpec đủ field pod/scheduling.
- FE toolkit `config/` bất-khả-tri-engine, tái dùng được: `KeyedPropertyList`/`PropertyList`, `PlacementSection`, `PodSection`/`ContainersMountsSection`, `RawJsonField`+`validityRegistry`/`ValidityProvider` (JSON sai → chặn Save).

## Kiến trúc

**A. Contract:** mở rộng `StarRocksConfig` — `fe_config`/`cn_config` (BTreeMap), `cn_autoscaling: Option<CnAutoscaling{min_replicas,max_replicas,target_cpu_percent}>`, `pod: Option<StarRocksPod{service_account,node_selector,annotations,pod_labels,env,tolerations,affinity,security_context,init_containers,sidecars,image_pull_secrets}>` (Value cho type k8s phức tạp). Default rỗng → engine cũ không ảnh hưởng.

**B. Provisioning (`orchestrator/starrocks.rs`):** (1) FE merge: parse base thành map có thứ tự, áp `fe_config` (admin đè, key mới thêm), **từ chối** override key egret-quản (run_mode/aws_s3_*/cloud_native_*/access_control/ssl_*/ports). (2) `starrocks_cn_configmap` → `{name}-cn-cm` `cn.conf` (chỉ khi non-empty) + gắn `starRocksCnSpec.configMapInfo`. (3) `autoScalingPolicy` trên CN khi có (version v2, min/max, metric CPU). (4) render pod fields lên CẢ FeSpec+CnSpec (merge cẩn thận với SSL init-container FE).

**C. Body/spec (`engine-mgmt/engines.rs`):** mở rộng `StarRocksConfigBody` (feConfig/cnConfig/cnAutoscaling/pod camelCase); `build_starrocks_spec` lưu (omit=preserve như #265); `build_starrocks_provision_req` đọc lại; **validate**: `feConfig` nhắm key egret-quản → 400.

**D. Frontend:** `useStarRocksConfigForm` thêm state/seed feConfig/cnConfig/cnAutoscaling/pod + vào PUT body + dirty (resolveConfig như #265). `StarRocksConfigTab` thêm nav FE config/CN config/Autoscaling/Pod & Containers, tái dùng KeyedPropertyList + form autoscaling nhỏ + PodSection/PlacementSection/ContainersMountsSection, bọc raw-JSON trong ValidityProvider (JSON sai chặn Save).

## Luồng dữ liệu

Sửa section → `save()` → `PUT /engines/{id}` (body starrocks mở rộng) → `update_starrocks_engine` → `build_starrocks_spec` (validate protected → 400) → persist → `build_starrocks_provision_req` → `StarRocksProvisioner::update` re-apply CR (+ ConfigMap FE/CN) → operator roll role. (Tái dùng đường update #265.)

## Kiểm thử

- **Unit:** merge FE-config (admin thắng; chặn protected key); `cn_configmap` chỉ khi non-empty; CR render `autoScalingPolicy` CN + pod fields FE+CN + SSL init-container FE sống-sót cạnh sidecar admin; `build_starrocks_spec` round-trip (omit=preserve); protected-key → 400; FE hook seed/build body mỗi section + JSON sai chặn Save.
- **Live:** StarRocks 3.4 thật — override `sys_log_level`/heap FE + CN HPA(min1 max3 70%CPU) + nodeSelector → Save → FE ConfigMap có key merge, HPA/`autoScalingPolicy` tồn tại, pod có nodeSelector.

## Rủi ro chỉ-live

1. Ngữ nghĩa key-trùng `fe.conf` (merge parse-and-replace tránh trùng).
2. `cn.conf` qua `starRocksCnSpec.configMapInfo` — operator mount giống FE?
3. `autoScalingPolicy` — operator tạo HPA, không đá nhau với `cn_replicas` tĩnh (autoscaling bật → replicas là sàn).
4. Merge pod field FE — `initContainers`/`storageVolumes` admin không đè keystore SSL.

## Phân rã (cho plan)

1. Contract + body/spec plumbing + protected-key 400. 2. Merge FE-config. 3. CN config + configMapInfo. 4. Autoscaling CN. 5. Pod & Containers (SSL-init-safe). 6. FE: FE/CN config + Autoscaling sections. 7. FE: Pod & Containers section. 8. CI + live smoke.
