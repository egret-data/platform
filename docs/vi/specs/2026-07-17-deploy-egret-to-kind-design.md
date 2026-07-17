# Deploy egret lên Kubernetes (kind) — Thiết kế

**Ngày:** 2026-07-17 · **Trạng thái:** Thiết kế — chờ duyệt · **Nhánh:** `feat/deploy-egret-kind` (off `main`)

> Bản chuẩn tiếng Anh: `docs/en/specs/2026-07-17-deploy-egret-to-kind-design.md`.

## Vấn đề

egret provision engine (Trino/StarRocks) **vào** k8s, nhưng bản thân egret luôn chạy **trên host** (`just serve` + kubeconfig local). Process trên host **không resolve được DNS nội cluster** (`<name>-fe-service.egret-engines.svc:9030`) → toàn bộ **data plane** (SQL Lab, dataset preview, query Trino/StarRocks, bootstrap delegation/root SQL) bị **chặn DNS ở dev**, chưa e2e được. Hiện **chưa có image backend, chưa có Helm chart** (chỉ có `frontend/Dockerfile` = nginx, `kernel-image/Dockerfile` = kernel notebook).

## Mục tiêu

1 lệnh dựng egret **trong** kind cluster có sẵn (backend + frontend + deps) → egret chạy in-cluster, resolve được engine DNS → data plane test được end-to-end. Phạm vi: **local kind cho e2e**, KHÔNG phải chart production.

Đây là **Project 1** trên 2. **Project 2** (spec riêng) = full automated e2e suite chạy trên deployment này.

## Ngoài phạm vi

- Không phải chart prod (không Ingress/TLS prod, không HA, không managed Postgres/OIDC ngoài). Chỉ values dev/kind.
- **Không đổi code app** — backend đã hỗ trợ in-cluster kube config sẵn; chỉ containerize + chart.
- Không làm image kernel notebook. Không wire CI (opt-in/manual như harness `deploy/k8s` hiện có).

## Bối cảnh — code đã cho sẵn (đã verify)

- **In-cluster kube tự động:** `egret-bin serve` gọi `LiveKube::in_cluster()` trước, fallback `from_kubeconfig()` (`main.rs:1324`). Pod có ServiceAccount token là chạy — chỉ cần **RBAC**, không cần config.
- **Image frontend tái dùng nguyên:** `nginx-prod.conf` proxy `/api/ → http://egret:8080`. Đặt tên Service backend là **`egret`** :8080 thì `frontend/Dockerfile` chạy **y nguyên**.
- **OIDC bật/tắt được:** `config.oidc.enabled` (main.rs:985); admin local (`admin@egret.local`/`Admin12345!`) login được khi OIDC tắt → **Mốc A**: deploy OIDC **tắt** (unblock data plane ngay); **Mốc B**: bật lại Keycloak cho các tính năng phụ thuộc OIDC (role theo group, JWT delegation).
- **Deps** (từ `docker-compose.dev.yml`): Postgres 16, MinIO, Keycloak 26; realm import `deploy/keycloak/egret-realm.json`.
- **Toolchain `stable`** → builder `rust:bookworm`. Migration **embed lúc compile** (sqlx), chạy lúc boot — image runtime không cần file migration.

## Kiến trúc

2 image, 1 Helm chart, deps bundle, deploy lên kind có sẵn.

- **A. Image backend** (`deploy/docker/backend.Dockerfile`, mới): multi-stage, context = repo root. Builder `rust:bookworm` (`cargo build --release -p egret-bin --locked`, cache layer deps) → runtime `debian:bookworm-slim` (`ca-certificates` + lib động cần thiết, check `ldd`), non-root, copy binary, `ENTRYPOINT ["egret","serve"]`, `EXPOSE 8080`.
- **B. Image frontend:** **tái dùng `frontend/Dockerfile` nguyên** (Service backend tên `egret`).
- **C. Helm chart** `deploy/helm/egret/`: namespace `egret-system` (egret+deps) + `egret-engines` (target provision); backend Deployment+Service(`egret`:8080)+ServiceAccount+ConfigMap(`egret.toml`)+Secret; frontend Deployment+Service **NodePort**; **deps bundle** (`deps.enabled`, default true): Postgres+MinIO(+PVC, seed bucket/DATABASE_URL)+Keycloak(realm import, chỉ khi OIDC bật). `values.yaml`: image repo/tag, deps.enabled, oidc.enabled (mặc định false ở Mốc A), NodePorts, admin creds, storage/secret.
- **D. RBAC** — mở rộng Role Trino-era: provisioner hiện tạo thêm **`batch/jobs`** (initPassword+keystore), **`autoscaling/v2 HPA`** (CN), **`starrocks.com` CR** (`starrocksclusters`). Role namespaced trong `egret-engines` (CR là namespaced) với `create/get/list/patch/delete` trên: apps/deployments · core/{services,configmaps,secrets,pods,pods/log,pvc} · batch/jobs · autoscaling/hpa · starrocks.com/starrocksclusters · core/events(read). RoleBinding gắn SA `egret`.
- **E. OIDC single-origin (Mốc B):** issuer URL phải GIỐNG NHAU cho browser (redirect) và backend (validate). Dùng **nginx-ingress** + host `egret.localtest.me` (→127.0.0.1): SPA `/`, Keycloak `/auth/…` → 1 origin 1 port(80) → issuer nhất quán. Browser: `localtest.me→127.0.0.1→`kind portmap→ingress; backend pod: cùng host → ingress ClusterIP qua **`hostAliases`**. `issuer_url=http://egret.localtest.me/auth/realms/egret`, Keycloak `KC_HOSTNAME` khớp. **Mốc A ship trước với `oidc.enabled=false`** (không cần ingress) để unblock data plane nhanh.
- **F. `just deploy-kind`:** build 2 image → `kind load docker-image … --name egret` → `helm upgrade --install egret deploy/helm/egret -n egret-system --create-namespace`. Thêm `just kind-undeploy`. `kind.yaml` thêm `extraPortMappings` cho NodePort (A) / ingress :80 (B).

## Luồng

`just deploy-kind` → load image → `helm install` dựng Postgres/MinIO(/Keycloak)+backend(SA+RBAC, `in_cluster()` tự động)+frontend NodePort. Browser→NodePort→SPA→`/api`→Service `egret:8080`. egret provision engine vào `egret-engines`; vì in-cluster → reach `<engine>-fe-service.egret-engines.svc:9030` → **SQL Lab/dataset/query chạy được**.

## Kiểm thử

- **Build/lint:** 2 image build; `helm lint`/`helm template` sạch (kubeconform nếu có).
- **Live (acceptance):** `just deploy-kind` → pods Ready → browser NodePort → login admin local (Mốc A) → provision **StarRocks** → Running → **SQL Lab `SHOW DATABASES`/`SELECT 1` ra rows** (unblock cốt lõi) → provision **Trino** + cross-catalog query. Mốc B: login OIDC qua `egret.localtest.me`, role theo group, query JWT-delegated. Screenshot stack + kết quả SQL Lab.

## Rủi ro

1. **OIDC dual-URL (Mốc B)** — khó nhất; giảm thiểu bằng ship Mốc A (OIDC tắt) trước + pattern `localtest.me` single-origin + `hostAliases`.
2. **Build image lâu/nặng** — cache layer deps; chấp nhận được (image local, không phải CI-hot).
3. **Thiếu lib runtime** — check `ldd`, thêm đúng lib (hoặc dùng `debian:bookworm` non-slim).
4. **Bootstrap bucket/DB** — tạo trước khi egret boot (initContainer + readiness; egret retry DB).
5. **RBAC drift** — engine type mới → Role phải thêm; gom verb ở 1 chỗ, comment gắn với provisioner.

## Project 2 (spec riêng) — full e2e, phân rã

Sau khi Project 1 live: viết full Playwright e2e trên harness `frontend/tests/e2e/` chạy trên deployment in-cluster. "Tất cả tính năng" là bề mặt lớn → Project 2 chia theo mảng: **auth/login** · **control-plane admin** (users/groups/roles/permissions/access-explorer/audit) · **engine provisioning** (Trino+StarRocks lifecycle) · **data plane** (SQL Lab/datasets/charts/dashboards/saved-queries) · **governance** (policies/masking/row-filter/classifications/Cedar) · **notebooks** (Daft). Mỗi mảng = plan bite-sized riêng (happy-path + edge chính). E2e chạy opt-in/manual, không vào CI infra-red.
