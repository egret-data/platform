# E2E: governance enforcement trên kind — Thiết kế

**Ngày:** 2026-07-18 · **Trạng thái:** Thiết kế — đã duyệt · **Nhánh:** `feat/e2e-governance-enforcement` (off `main`)

> Bản chuẩn tiếng Anh: `docs/en/specs/2026-07-18-e2e-governance-enforcement-design.md`.

## Vấn đề

Project 2 slice 1 (data-plane, PR #273) + 2 (auth+control-plane admin, PR #274) đã xong ([[egret-e2e-dataplane-kind]]). Slice tiếp = **governance enforcement** — assertion crown-jewel của data-plane: một policy (column-mask / row-filter / deny) **thực sự đổi output** của query governed. Ranger enforcement đã proven thủ công (P3.2 tag masking #232, StarRocks #259) nhưng chưa bao giờ thành e2e tự động.

Đây là slice nặng nhất: cần Trino engine bound Ranger, bảng governed có data, policies, và query — kế thừa độ rung engine-warmup của slice 1 cộng độ trễ Ranger policy-poll. Chọn có chủ đích (facet B, enforcement thật) thay vì admin-UI CRUD tất định (facet A).

## Mục tiêu

Một spec Playwright lifecycle, chạy qua biến thể `just e2e-kind` bật Ranger, nhắm in-cluster kind deploy: provision Trino engine bound-Ranger, seed bảng Iceberg governed, tạo policy qua egret policy admin API, và assert query as admin trả về **giá trị cột bị mask + rows đã row-filter** — chứng minh Ranger enforcement end-to-end. Mô hình chạy như cũ: kind canonical, opt-in `EGRET_E2E_KIND=1`, không trong GitHub CI.

## Ngoài phạm vi

- Governance **admin-UI CRUD** tất định (facet A: form quản lý policy/classification) — slice sau.
- Enforcement hai-user (admin-raw vs member-masked) — slice này nhắm **chính username admin** (single-user), tránh session thứ 2; biến thể hai-user để refine sau.
- StarRocks Ranger enforcement, Cedar BI ABAC, tag-based masking — chỉ Trino resource-based mask/row-filter/deny.
- Provisioning-as-test, notebooks, BI/SQL-Lab — slice riêng.

## Bối cảnh — bề mặt đã grounding (verify)

- **egret CHÍNH LÀ Ranger admin.** Với `config.ranger.enabled=true`, egret mount `/service` (`ranger_router`, `crates/egret-server/src/lib.rs:174`) — bề mặt Ranger Admin REST mà Trino plugin poll (`/service/plugins/policies/download/{service}`). **Không deploy Ranger server riêng.** `config.ranger.enabled` default **false** (`egret-config`), deploy kind hiện tại tắt → engine provision **allow-all** (`ranger disabled; ... allow-all` trong logs). `auth_token=""` mở `/service` anonymous cho stock plugin.
- **Provision engine bound-Ranger.** Khi `EngineMgmtState.ranger_enabled=true` (từ `config.ranger.enabled`, `main.rs:1481`) và không có admin-chosen service, engine create **auto-bind** một Ranger profile/service mới (`engines.rs:1922`), và (PR #272) Trino provision với `access-control.name=ranger` + `ranger-trino-security.xml` trỏ plugin tới `/service` in-cluster của egret.
- **Policy admin API** (`crates/egret-policy/src/admin`, mount tại `/api/control-plane`, tới được qua ingress `/api`): `GET/POST /policies` + service-def / principals / resource-lookup. Policy tạo ở đây được serve cho plugin qua `/service`. **`/service` KHÔNG tới được qua browser ingress** (nginx route path non-`/api` về SPA), nên e2e PHẢI dùng `/api/control-plane/policies`, không phải `/service` trực tiếp.
- **Query chạy AS caller** (impersonation, `X-Trino-User` = username user đã login; `execution/routes.rs:124`). Nên enforcement per-user — nhắm username admin bằng policy mask/row-filter làm chính query của admin hiện enforcement.
- **Recipe thủ công** (`docs/en/testing/2026-06-18-trino-iceberg-ranger-manual-test.md`) là tham chiếu proven: default-deny cần global `execute` trên `queryid=*` + `impersonate` trên `trinouser=*`; rồi `select` trên bảng; rồi policy column-`MASK` và `rowFilterInfo.filterExpr`. Tên catalog Trino là `iceberg`; plugin poll ~5s.
- **Reuse** helper slice 1 (`provisionStorageAndWarehouse`, `provisionTrinoEngine`, `waitForEngineReady`, `enableEngineJwt`, `assignEngineToWorkspace`, helper query-submit, `csrf`) + hình dạng runner `just e2e-kind`.

## Kiến trúc

Một spec lifecycle dài + biến thể deploy bật-Ranger + helper nhỏ. Provisioning + policy setup qua egret API (`page.request`); assertion enforcement là query qua SQL Lab backend.

### A. Deploy — egret bật Ranger

- **Chart** (`deploy/helm/egret`): thêm value `ranger.enabled` (default false) → `configmap-env.yaml` phát `EGRET_RANGER__ENABLED` + `EGRET_RANGER__AUTH_TOKEN=""` khi set. (Anonymous `/service` cho stock plugin.)
- **Runner**: recipe `deploy-kind-governance` (hoặc mở rộng `deploy-kind-oidc`) = `helm upgrade --install ... --set oidc.enabled=true --set ingress.enabled=true --set ranger.enabled=true`, rồi `wait-ready.sh`. Spec e2e chạy dưới `EGRET_E2E_KIND=1` như các slice khác.

### B. Spec — `frontend/tests/e2e/governance-kind.spec.ts`

Gate: skip trừ khi `EGRET_E2E_KIND=1`; `test.setTimeout(900_000)`; cleanup `try/finally`; login một lần (shared page, serial — bài học rate-limit login slice 2). Flow (as admin, qua `page.request` trừ khi ghi chú):

1. **Provision** storage → warehouse → Trino engine (Ranger auto-bound vì deploy bật-ranger) → `waitForEngineReady` → `enableEngineJwt` → `waitForEngineReady` → assign workspace. (Helper slice 1.)
2. **Seed bảng governed** qua SQL Lab query backend (`POST /api/ws/{ws}/queries`, retry qua coordinator warmup): `CREATE SCHEMA` → `CREATE TABLE report(id, item, country, amount)` → `INSERT` vài rows (một số `country='US'`, một số không). Poll mỗi cái tới `finished`.
   - Cần globals default-deny đã cho phép admin chạy DDL/DML → tạo policy đó trước (step 3), hoặc seed trước khi enforcement bật (pin ở plan; đơn giản nhất: tạo global execute/impersonate + admin-allow rộng, chờ poll, rồi seed).
3. **Tạo policy** qua `/api/control-plane/policies` (trên profile của engine auto-bound): global `execute` (`queryid=*`) + `impersonate` (`trinouser=*`); table `select` cho username admin; **column-MASK** trên `item` cho admin; **row-filter** `country='US'` cho admin. Resolve username admin qua `/api/me`. Chờ ~8–10s cho plugin poll.
4. **Assert enforcement**: `SELECT id, item, country FROM iceberg.report ORDER BY id` as admin → kết quả chỉ có rows `country='US'` (row-filter) VÀ cột `item` bị mask (không phải giá trị gốc). So với data đã seed.
5. **Cleanup** (`finally`): xoá policies, drop bảng, xoá engine.

### C. Helper

Thêm vào `helpers.ts` khi cần: `runQuery(request, wsSlug, dbName, sql)` submit + poll + trả result JSON (tổng quát hoá đường API của `runSqlInLab` slice 1, dùng cho DDL/DML + assertion), và `createRangerPolicy(request, csrf, ...)` wrapper mỏng nếu DTO policy dài dòng.

### D. Docs

Thêm mục "Governance enforcement e2e" vào `frontend/tests/e2e/README.md`: cần deploy bật-ranger (`deploy-kind-governance`), nó chứng minh gì (Ranger mask + row-filter trên query live), và plugin poll Trino Ranger thêm ~5–10s.

## Kiểm thử / acceptance

Không phải "đã viết test" — **chạy thật** qua deploy bật-ranger và xác nhận spec PASS (query admin hiện `item` masked + chỉ rows `US`). Unit gates (`npm run lint`/`typecheck`/`test`/`build`) giữ xanh; spec opt-in.

## Rủi ro

1. **Slice nặng nhất** — engine warmup (~40s) + Ranger plugin poll (~5–10s) + propagation; kỳ vọng nhiều run để hội tụ (như slice 1). Timeout rộng + retry.
2. **Write path (INSERT) qua catalog** — seed data qua engine exercise Iceberg REST write path (empty-location guard, reserved properties); có thể lộ vấn đề mà slice-1 read-only không thấy.
3. **Thứ tự default-deny** — Ranger bật, admin cần policy execute/impersonate/select TRƯỚC mọi query (kể cả DDL). Đúng thứ tự policy-rồi-seed (tạo globals + admin-allow trước, poll, rồi seed).
4. **DTO policy admin API** — DTO policy egret khác JSON Ranger `/service` thô trong recipe thủ công; pin body `POST /api/control-plane/policies` chính xác (shape access / mask / row-filter) lúc writing-plans.
5. **Chart change** — slice này đụng Helm chart (ranger config), khác các slice thuần-test.

## Project 2 còn lại sau slice này

governance admin-UI CRUD (facet A) · provisioning-as-test · notebooks (Daft) · BI/SQL-Lab consumer UI · biến thể enforcement hai-user + StarRocks + Cedar. Mỗi cái spec → plan riêng.
