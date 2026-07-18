# E2E: data-plane money-path trên kind — Thiết kế

**Ngày:** 2026-07-18 · **Trạng thái:** Thiết kế — đã duyệt · **Nhánh:** `feat/e2e-dataplane-kind` (off `main`)

> Bản chuẩn tiếng Anh: `docs/en/specs/2026-07-18-e2e-dataplane-kind-design.md`.

## Vấn đề

egret giờ chạy **in-cluster trên kind** (Project 1, PR #270/#271/#272 đã merge) và money-path data-plane — OIDC → engine JWKS → JWT → TLS:8443 → **impersonation** (`SELECT current_user` = caller) → query — đã được **chứng minh bằng tay** qua Playwright MCP. Nhưng **chưa thành test tự động**. Harness e2e hiện có (`frontend/tests/e2e/`) nhắm **host dev stack** (Vite :5173 + host `serve`), không phải in-cluster, và chỉ phủ notebook + embedded-catalog. Đây là slice đầu của **Project 2** (full automated e2e), decompose theo mảng: auth / control-plane / provisioning / data-plane / governance / notebooks.

## Mục tiêu

Một spec Playwright drive **trọn lifecycle data-plane trên in-cluster kind deploy** và assert impersonation chạy end-to-end, kèm runner `just e2e-kind`. Khoá lại — như một regression gate lặp lại được — đúng chuỗi ta phải chứng minh bằng tay.

Mô hình chạy (đã chốt): **kind là target canonical, local opt-in.** Full CI (`ci.yml`) giữ nguyên unit-only (GitHub CI kinh niên infra-red); e2e là gạc local riêng, chạy trên kind cluster của dev.

## Ngoài phạm vi

- 5 mảng Project-2 còn lại (auth, control-plane, provisioning-as-test, governance, notebooks) — mỗi cái là slice sau.
- kind-in-GitHub-CI (nặng; ngược với thực tế CI luôn infra-red).
- StarRocks data-plane e2e (Trino trước; StarRocks follow-on).
- Không đổi code app/backend — slice này chỉ harness test + runner + docs.

## Bối cảnh — cái đã có sẵn (đã verify)

- **Harness:** `frontend/tests/e2e/` — Playwright, `baseURL` env-driven (`EGRET_E2E_BASE_URL`, mặc định `http://localhost:5173`), `workers:1`, không parallel, timeout 90s. Spec gate bằng env flag (`EGRET_E2E_CATALOG=1`).
- **Login helper** (`helpers.ts`): `loginViaOidc` (Keycloak SSO) và `loginLocalAdmin` (break-glass local admin, xử lý toggle). Kind deploy seed `admin@egret.local` / `Admin12345!` qua `egret init`, nên `loginLocalAdmin` chạy được trên kind kể cả khi OIDC bật.
- **Pattern API-trong-test:** `embedded-catalog.spec.ts` đã provision qua `page.request` (chia session cookie đã login) với helper `csrf(context)` double-submit cục bộ. Slice này tái dùng + tổng quát hoá pattern đó.
- **Endpoint provisioning** (bám `crates/egret-engine-mgmt/src/engines.rs`): `POST /engines`, `PUT /engines/{id}` (jwt block resolve trên create+update bởi `resolve_auth`), `DELETE /engines/{id}`, `POST /engines/{id}/restart`, `PUT /engines/{id}/workspaces` (set full assignment set) hoặc `POST /api/admin/workspaces/{id}/engines`. Storage + warehouse (catalog) nằm ở control-plane router (pin DTO chính xác lúc writing-plans).
- **Recipe deploy** (`justfile`): `deploy-kind`, `k8s-up`, `kind-undeploy`, `ingress-install`, `coredns-rewrite`. Deploy tại `http://egret.localtest.me`.
- **Bài học xương máu (từ lần proof tay, để thiết kế tránh):** single kind node ~93% mem → engine test đặt 1Gi / workers=1 + cleanup sau; toggle+save JWT trên UI có race cùng-lần-gọi (làm bằng API tuần tự, không toggle UI); Monaco SQL Lab chặn click trực tiếp → set query qua `monaco.editor.getModels()[0].setValue(...)`; Trino non-Ranger fresh giờ tự ghi+mount `access-control.name=allow-all` (PR #272) → impersonation chạy sẵn khi ranger off.

## Kiến trúc

Hybrid: **provision qua egret API, assert qua browser.** Một spec chạy dài drive trọn lifecycle; recipe `just e2e-kind` đưa deploy tới Ready rồi chạy.

### A. Spec — `frontend/tests/e2e/dataplane-kind.spec.ts`

Gate: skip trừ khi `EGRET_E2E_KIND=1`. `test.setTimeout(600_000)` (provision engine mất vài phút). Một test, trọn lifecycle:

1. **Login** `loginLocalAdmin(page)`; lấy `page.request` + `csrf(context)`.
2. **Provision qua API** (idempotent, tên resource mang suffix theo run từ `EGRET_E2E_RUN_ID` hoặc timestamp để chạy lại không đụng):
   - Storage conn → MinIO in-cluster (`http://minio:9000`, bucket `warehouse`, key `egret` / `egretsecret`).
   - Warehouse (catalog) trên conn đó.
   - `POST /engines` Trino: **workers=1, memory 1Gi**, wire catalog đó, **Ranger off** (→ allow-all theo PR #272 → cho phép impersonation).
3. **Poll tới Ready:** `waitForEngineReady(request, id)` — `GET /engines/{id}` tới khi coordinator báo Ready (~5s poll, cap ~4 phút).
4. **Bật JWT:** `PUT /engines/{id}` với jwt block → poll tới Ready lại (coordinator re-provision). API tuần tự tránh race toggle+save của UI.
5. **Assign workspace:** `PUT /engines/{id}/workspaces` gán workspace seed (`default` từ `egret init`, hoặc `demo`).
6. **Assert qua browser:** mở `/explore/{ws}/sql-lab`, set Monaco `SELECT current_user` qua `setValue`, chọn engine ở Database dropdown, Run, assert panel kết quả chứa caller (`admin`) — **không phải** `egret_delegator` — chứng minh impersonation.
7. **Cleanup (`afterAll`):** `DELETE /engines/{id}` + xoá warehouse/storage để giải phóng RAM node. Idempotent; nuốt lỗi nếu đã xoá.

**Helper dùng chung** thêm vào `helpers.ts`: `csrf(context)`, `provisionTrinoEngine(request, opts)`, `waitForEngineReady(request, id)`, `runSqlInLab(page, ws, sql)`, `deleteEngine(request, id)`. `embedded-catalog.spec.ts` sửa để import `csrf` từ `helpers.ts` (DRY) thay vì định nghĩa cục bộ.

### B. Runner — `just e2e-kind`

```
# Opt-in data-plane e2e nhắm in-cluster kind deploy (egret.localtest.me).
# Prereq một lần: just k8s-up && just ingress-install && just coredns-rewrite
e2e-kind: deploy-kind-oidc
    ./deploy/k8s/wait-ready.sh
    cd frontend && EGRET_E2E_KIND=1 \
      EGRET_E2E_BASE_URL=http://egret.localtest.me \
      npx playwright test dataplane-kind
```

- **`deploy-kind-oidc`** — anh em của `deploy-kind`, chạy `helm upgrade --install ... --set oidc.enabled=true --set ingress.enabled=true` (money-path cần JWT/OIDC = Milestone B). `deploy-kind` gốc (OIDC off) giữ nguyên.
- **`deploy/k8s/wait-ready.sh`** (mới) — `kubectl wait --for=condition=Ready` cho pod egret + keycloak, rồi poll `curl -sf http://egret.localtest.me/api/auth/config` tới 200 (OIDC discovery hội tụ qua khỏi race Keycloak-not-ready 503). Thay việc canh pod bằng tay.
- **Idempotent theo thiết kế:** `helm upgrade --install` chạy lại vô hại; spec tự cleanup engine; tên resource theo run → `just e2e-kind` lặp lại được trên cùng cluster.
- **KHÔNG tạo cluster** — `e2e-kind` cần kind `egret` cluster + ingress + coredns-rewrite có sẵn (tránh recreate ~14 phút + mất state); fail sớm với chỉ dẫn rõ nếu thiếu cluster.

### C. Docs

- `frontend/tests/e2e/README.md` — thêm mục "Data-plane e2e on kind": prereq một lần, `just e2e-kind`, env override, cái nó chứng minh.
- Spec này (EN chuẩn + VI tham chiếu).

## Kiểm thử / acceptance

Không phải "đã viết test" — **chạy thật.** Sau khi code xong, chạy `just e2e-kind` trên kind cluster hiện có và xác nhận spec PASS (`SELECT current_user` = `admin`), chụp Playwright report. Unit gate (`cargo fmt/clippy/test`, frontend `lint`/`typecheck`) giữ xanh; spec mới opt-in, không chạy trong `npm test`.

## Rủi ro

1. **RAM node ~93%** (rủi ro cao nhất) — engine 1Gi + cleanup `afterAll` bắt buộc; README ghi cách xoá engine cũ nếu node vẫn nghẽn.
2. **Provision chậm / timeout** — poll cap hợp lý (~4 phút/lần Ready), `test.setTimeout(600s)`.
3. **Selector Monaco / dropdown trôi theo UI** — gom vào `runSqlInLab` để một chỗ sửa; ưu tiên role/text selector bền.
4. **OIDC discovery race** — `wait-ready.sh` poll `/api/auth/config` 200 trước khi test chạy.

## Project 2 còn lại (slice riêng)

Sau slice này: auth/login · control-plane admin · provisioning-as-test (full UI provisioning click-through) · governance (policies/masking/row-filter/Cedar) · notebooks (đã phủ một phần). Mỗi cái spec → plan → impl riêng. kind-in-GitHub-CI và StarRocks data-plane e2e để sau.
