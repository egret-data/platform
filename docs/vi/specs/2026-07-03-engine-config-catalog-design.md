# Config surface + multi-catalog, Trino trên manifest thường — thiết kế (Phase 1.5, 2026-07-03)

Bản EN (canonical): `docs/en/specs/2026-07-03-engine-config-catalog-design.md`.
Nối tiếp Phase 1: `docs/en/specs/2026-07-03-engine-provisioning-design.md`.

**Mục tiêu:**
1. **Pivot Trino khỏi Stackable operator sang manifest K8s thường, stateless** do
   egret tự template (đúng hình dạng chart `trinodb/trino` chính thức sinh ra) —
   Deployment, không StatefulSet, không PVC, không cần operator.
2. **Mở rộng config surface** (curated + raw escape hatch).
3. **Cluster nhiều catalog** (egret warehouses + Postgres + tpch/tpcds + generic).

**Vì sao pivot:** Stackable model Trino là StatefulSet + per-pod TLS + PVC `data-`
persistent. Trino gần như stateless nên PVC chỉ gây phiền — worker crash để lại PID
lock cũ trong data dir, sống qua restart → crashloop tự lặp. Deployment (model Helm
chính thức) stateless: restart thoải mái, không PVC, không lock.

**Non-goals (1.5):** egret→Trino reverse proxy / expose ra ngoài = **Phase 1.6**
(spec riêng, hỗ trợ cả path + subdomain, chọn khi cấu hình); StarRocks/RisingWave
(operator riêng sau); rotation; autoscaling.

## Mechanism — egret template manifest (rewrite `trino.rs`)
Giữ nguyên toàn bộ Phase 1 (trait `EngineProvisioner`, `KubeClient`, data model,
routes, auto-wire, UI). Chỉ rewrite `crates/egret-orchestrator/src/trino.rs`: thay CR
bằng apply object thường qua `KubeClient`:
- Deployment coordinator (1) + Deployment worker (N) + Service ClusterIP (:8080)
- ConfigMap `config` (config.properties + jvm.config + node.properties)
- ConfigMap `catalogs` (mỗi catalog 1 file `<name>.properties`)
- Secret (nếu catalog cần creds) → mount env, properties dùng `${ENV:VAR}`

**Trino không hot-reload catalog** → đổi config/catalog ⇒ re-render ConfigMap + đổi
annotation `checksum/config` trên Deployment ⇒ rolling restart (rẻ vì stateless). UI
báo "restarting to apply".

## Config surface
- Curated → key trong config.properties/jvm.config: jvm heap, query.max-memory,
  query.max-memory-per-node.
- Raw escape hatch → map key→value ghi thẳng config.properties + env (curated thắng
  khi trùng key).

## Multi-catalog
Bảng `engine_catalogs` (`id, engine_id FK CASCADE, name, kind, config jsonb,
secret_ref?, is_primary`). Catalog primary (egret-warehouse auto-wire) = row đầu tiên
lúc provision → đồng nhất. Kinds: `egret-warehouse` (iceberg REST + creds Secret),
`postgresql` (host/port/db/user + password Secret), `tpch`/`tpcds` (zero-config),
`generic` (connectorName + properties). Creds **lấy từ egret Secrets module**
(`resolve_plaintext`) → egret tạo k8s Secret → env → `${ENV:...}`. Không plaintext
trong spec/ConfigMap.

## Backend / Frontend / Test
- Routes: `PUT /engines/{id}` (config → re-render + roll); `GET/POST /engines/{id}/catalogs`,
  `DELETE /engines/{id}/catalogs/{cid}` (không xoá primary).
- Frontend EngineDetail: **Config section** (curated + raw editor) + **Catalogs
  section** (list + Add-catalog theo kind + remove).
- Test: unit (fake kube, kiểm manifest render) + live kind **không operator** (chỉ
  apply Deployment; add tpch → cross-catalog join). Nhẹ hơn harness Phase 1.

## Bỏ so với Phase 1
❌ Stackable Trino operator (prereq) · ❌ PVC · ❌ PVC-cleanup-on-delete · ❌ TrinoCatalog/TrinoCluster CR.
`engine_catalogs` cascade khi xoá engine.

Chi tiết đầy đủ xem bản EN.
