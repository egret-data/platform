# E2E: provision engine qua UI trên kind — Thiết kế

**Ngày:** 2026-07-18 · **Trạng thái:** Thiết kế — đã duyệt · **Nhánh:** `feat/e2e-provisioning-ui` (off `main`)

> Bản chuẩn tiếng Anh: `docs/en/specs/2026-07-18-e2e-provisioning-ui-design.md`.

## Vấn đề

Project 2 slice 1–3 provision engine qua **egret API** (helper nhanh). Chưa slice nào exercise **UI provision engine ở control-plane** — form "Add engine", status badge live, và luồng delete mà platform admin thực sự click. Slice 4 ("provisioning-as-test") phủ đường browser đó.

Đây là hybrid: form tạo tất định + async chờ engine đạt `running` (k8s provisioning thật). Nó tránh phần flaky nhất của slice 1 (không JWT toggle, không query warmup) — chỉ provision-tới-running — nên dễ hơn full data-plane.

## Mục tiêu

Một spec Playwright drive UI provision engine ở control-plane trên in-cluster kind deploy: điền form "Add engine" cho cụm Trino, chờ status badge của engine đạt **running**, rồi xoá qua UI — chứng minh UI provisioning chạy end-to-end. Mô hình chạy như cũ: kind canonical, opt-in `EGRET_E2E_KIND=1`, không trong GitHub CI.

## Ngoài phạm vi

- StarRocks provisioning (variant sau — thêm operator + CR).
- Storage / warehouse provisioning qua UI (seed qua API ở đây; UI của chúng có thể là slice sau).
- Engine config editor sâu, bật JWT, assign workspace, query (slice 1/3 đã phủ).
- RisingWave (chưa provision được).

## Bối cảnh — bề mặt đã grounding (verify)

- **`ControlPlaneEngines.tsx`** (`/control-plane/engines`, gate `RequirePermission perm="engine:read"`): Tabs bar per engine-type (Trino / StarRocks / RisingWave) set `activeType`; nút **"Add engine"** (disabled trừ khi active type provision được) mở **Dialog** form tạo với `name`, một **warehouse** `Select`, và các input resource (`coordinatorReplicas`, `workerReplicas`, `cpuMin`, `cpuMax`, `memory` — default `cpuMin:"1"`, `cpuMax:"2"`, `memory:"4Gi"`). Submit gọi `createEngine` (`POST /engines`). Mỗi row list hiện `engine.name`, `engine.namespace`, type Badge, và **`<EngineStatusBadge>`** live (per-row status poll) render phase (`running` / `provisioning` / …); summary strip hiện running / not-running.
- **Prereq:** form chọn một **warehouse** — phải có một cái. Seed storage + warehouse qua API (reuse slice-1 `provisionStorageAndWarehouse`) với reserved bucket `e2e`.
- **Provision-tới-running không cần S3/JWT** — engine chỉ cần pods Ready; slice-1 chứng minh Trino 1Gi/1-worker đạt `running` ~1–2 phút. Dùng resource nhỏ trong form để tránh OOM / chờ lâu.
- **Login rate-limited** (bài học slice 2) → login một lần (shared page, serial).
- Reuse helper slice 1 (`csrf`, `provisionStorageAndWarehouse`, `deleteEngine` cho cleanup `finally`) + runner kiểu `just e2e-kind` (deploy OIDC bất kỳ; ranger state không liên quan tới việc đạt `running`).

## Kiến trúc

Một spec + API setup cho warehouse; lifecycle engine drive qua browser UI.

### A. `frontend/tests/e2e/provisioning-ui-kind.spec.ts`

Gate: skip trừ khi `EGRET_E2E_KIND=1`; `test.setTimeout(600_000)`; login một lần (shared page, serial); cleanup `try/finally` (xoá engine qua API như lưới an toàn nếu UI delete không chạy).

Flow:
1. **Setup (API):** `provisionStorageAndWarehouse` (bucket `e2e`) → một warehouse đã provisioned để form chọn.
2. **Mở form tạo:** `goto /control-plane/engines`; đảm bảo tab Trino active; click **"Add engine"** → dialog mở.
3. **Điền + submit:** set `name` = `e2e-ui-${RUN_ID}`; chọn warehouse đã seed trong warehouse `Select`; set resource **nhỏ** (`workerReplicas`=1, `memory`=`1Gi`); submit. Dialog đóng, engine hiện trong list.
4. **Chờ running:** poll status badge của engine row tới khi đọc **running** (reload/poll, cap ~4 phút — k8s provisioning thật + Trino startup). Assert row hiện `running`.
5. **Xoá qua UI:** mở actions của engine row → Delete → confirm; assert row biến mất khỏi list.
6. **Cleanup (`finally`):** `deleteEngine` best-effort qua API (phòng khi bước UI delete fail), để không leak engine.

### B. Runner

Chạy dưới flow opt-in sẵn có — `EGRET_E2E_KIND=1` vào deploy đang chạy (vd cái `just e2e-kind` dựng). Ghi trong README; không cần biến thể deploy mới (dùng `EGRET_E2E_BUCKET=e2e`).

### C. Docs

Thêm mục "Engine provisioning via the UI" vào `frontend/tests/e2e/README.md`: nó drive gì (form Add-engine → status `running` → delete), ghi chú reserved-bucket, và chờ ~1–2 phút engine lên.

## Kiểm thử / acceptance

Không phải "đã viết test" — **chạy thật** và xác nhận spec PASS (Trino tạo qua UI đạt `running`, rồi xoá). Unit gates (`lint`/`typecheck`/`test`/`build`) giữ xanh; spec opt-in.

## Rủi ro

1. **Async provisioning wait** — engine mất ~1–2 phút đạt `running`; poll status badge với cap rộng; resource nhỏ (1Gi/1 worker) giữ nhanh + tránh OOM.
2. **Form selectors** — warehouse `Select` (Radix combobox) + resource inputs + submit; gom lại + finalize theo DOM live (như slice trước). Warehouse Select không được nhầm với workspace switcher trên nav (bài học slice 1).
3. **Status-badge polling** — `<EngineStatusBadge>` poll per row; spec có thể cần reload hoặc chờ text badge lật thành `running`.
4. **Reserved bucket** — cần `EGRET_E2E_BUCKET=e2e` (bucket `warehouse` có lakekeeper orphan); ghi tài liệu.

## Project 2 còn lại sau slice này

governance admin-UI CRUD (facet A) · notebooks (Daft) · BI/SQL-Lab consumer UI · biến thể StarRocks + storage/warehouse-UI provisioning. Mỗi cái spec → plan riêng.
