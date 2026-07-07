# Data Governance — Policies (Ranger, Trino trước) — Thiết kế

**Ngày:** 2026-07-06 · **Trạng thái:** Thiết kế — chờ duyệt · **Nhánh:** `feat/data-governance-policies` (off main)
**Chương trình:** Sub-epic 1 của menu **Data Governance** (Policies ▶ rồi Data Lineage). Spec này **chỉ** cho Policies. Data Lineage (OpenLineage + graph, tham chiếu Marquez) là spec riêng sau. Tham chiếu: **Apache Ranger admin console** (`data-portal/ranger/`) và plugin Trino Ranger native (`data-portal/trino/plugin/trino-ranger`).

> Bản tiếng Anh là bản chuẩn: `docs/en/specs/2026-07-06-data-governance-policies-design.md`.

## Mục tiêu
Xây trang **Policies** trong Control Plane để platform admin tạo/sửa/xóa policy Apache Ranger — **Access, Column-masking, Row-filter** — cho một hoặc nhiều **profile** engine, và để Trino enforce. Trino trước; editor **lái theo service-def** nên StarRocks sau chỉ cần thêm một service-def mới.

## Bối cảnh hiện tại (đã kiểm chứng)
- **`egret-authz` (`AuthzStore`)** đã mô hình đầy đủ Ranger và **multi-service**; mọi lần ghi policy đều **bump `authz_services.policy_version`** → engine đang poll tự tải lại. Policy lưu nguyên `policy_jsonb`.
- **`egret-ranger`** là surface Ranger-Admin-REST-**compatible**, mount ở **`/service`**, gated bằng **bearer token dùng chung** — endpoint **cho máy** (plugin Trino poll `/service/plugins/policies/download/{service}`), **không** session-auth, không gọi từ browser.
- **Service-def Trino** đã seed (`seed_trino`, `TRINO_DEF_NAME="trino"`): có `accessTypes`, `dataMaskDef`, `rowFilterDef`, resources `catalog → schema → table → column`. **Chưa có StarRocks def.**
- **`egret-controlplane`** mount `/control-plane`, CSRF, gated `require_platform_admin`; pattern **module-per-domain** (`engines/storage/secrets/warehouses`). Engines hiện **chỉ Trino**, **chưa liên kết** Ranger service.
- **Identity**: `egret-auth` có `UserRepo.list` + bảng `groups`; chưa có endpoint principals.
- **Frontend**: `Sidebar.tsx` đã có nhóm **Data Governance** trong `EXPLORE_GROUPS` với **Policies** + **Data Lineage**, cả hai đang `<Placeholder>`. API: mỗi domain 1 file `.ts` + react-query hooks.
- **Trino Ranger là native**: plugin `trino-ranger` có sẵn trong bản Trino chuẩn — **không cần custom image/jar**. Bật bằng `access-control.properties` (`access-control.name=ranger`, `ranger.service.name`, `ranger.plugin.config.resource=ranger-trino-security.xml`) + `ranger-trino-security.xml` trỏ `ranger.plugin.trino.policy.rest.url` về `/service` của egret. Dev Trino hiện dùng `access-control.name=file`; enforce Ranger thật hiện chỉ chạy qua harness Java `tests/ranger/`.

## Quyết định (đã chốt với người dùng)
1. **Policies trước**, Data Lineage sau (spec riêng).
2. **Scope = Global / Control Plane**, chỉ `platform_admin`. Mục menu **chuyển** từ Explore sang Control Plane.
3. **Đủ 3 loại policy** — Access + Masking + Row-filter — editor **def-driven**.
4. **Principals** lấy từ users + groups của egret (picker combobox), không free-text.
5. **Multi-profile**: 1 profile = 1 Ranger **service** có **vòng đời độc lập**. Engine link tới profile (**nhiều engine → một profile**, nullable). Khi provision, admin **tạo profile mới** (mặc định) **hoặc link profile có sẵn** — nhiều cụm Trino share chung policy set. Xóa engine thì **giữ** profile + policy (đứng độc lập, link lại được).
6. **Ranh giới enforce = Lớp A ngay + Lớp B sinh config-artifact** (§4). Không đụng image Trino.

## Kiến trúc — phương án đã chọn
API mới **session-auth, platform-admin** trong `egret-controlplane` đọc/ghi **cùng `AuthzStore`** với router máy `/service`. Loại: gọi `/service` từ browser (auth máy, không RBAC, rò wire camelCase); proxy forward (thừa lớp). Chung store nên policy tạo từ UI được plugin Trino nhận qua `/service/plugins/policies/download/{service}` sẵn có (version bump lo cache).

## 1. Backend — module mới `egret-controlplane/src/policies.rs`
Mount `/control-plane`, CSRF + `require_platform_admin`:
- `GET /control-plane/policies?service=&type=&resource=&q=` — list, parse `policy_jsonb` → **DTO tóm tắt typed**.
- `POST /control-plane/policies` — tạo; nhận DTO editor → **serialize sang Ranger JSON hợp lệ** (`policyType` 0/1/2; `resources`; `policyItems | dataMaskPolicyItems | rowFilterPolicyItems`).
- `GET/PUT/DELETE /control-plane/policies/{id}`.
- `GET /control-plane/policies/services` — list profile (service) + engine(s) đang link (0..n) + type; hỗ trợ create/link từ UI.
- `GET /control-plane/policies/service-def/{name}` — trả `resources / accessTypes / dataMaskDef.maskTypes / rowFilterDef` để editor render (điểm khiến editor def-driven).
- `GET /control-plane/principals?q=&kind=` — typeahead users + groups.
Logic **(de)serialize Ranger↔DTO** là lõi, ở submodule có unit test round-trip cho cả 3 loại.

## 2. Gắn profile ↔ engine (Lớp A)
Profile (Ranger service) có **vòng đời riêng**, tách khỏi engine. Link engine→profile là **nhiều-một, nullable** (`ON DELETE SET NULL`): nhiều engine enforce **cùng** policy set, profile sống lâu hơn engine.
- **Migration**: thêm `authz_service_id BIGINT NULL REFERENCES authz_services(id) ON DELETE SET NULL` vào bảng engines. **Không** `UNIQUE` (cho phép share). Nhớ chạm `metastore/src/lib.rs` để migrator re-embed.
- **Khi provision engine**: request mang **lựa chọn profile** —
  - `create` (mặc định): `create_service(name = engine slug, type = "trino")`, seed def `trino` nếu thiếu, set link; hoặc
  - `link`: set `authz_service_id` tới service id có sẵn (validate: tồn tại + type khớp kind engine). Không tạo service mới.
  Idempotent.
- **Relink / detach**: trang governance của engine đổi `authz_service_id` (link profile khác, hoặc gỡ = null) sau provision.
- **Khi xóa engine**: `ON DELETE SET NULL` (hoặc unlink chủ động) **giữ** profile + policy; service đứng độc lập, engine khác **link lại được**. Không bao giờ cascade-delete service.
- **Profile selector** list từ `/control-plane/policies/services`; mỗi service hiện **engine(s) đang dùng** (0..n) + service standalone (như `trino` seed).
- StarRocks: ngoài scope; sau này lựa chọn tương tự tạo/link service `starrocks` từ `seed_starrocks` — **UI không đổi** vì def-driven.

## 3. Frontend
- **Chuyển menu** (`Sidebar.tsx`): bỏ **Policies** khỏi `EXPLORE_GROUPS`; thêm nhóm **Data Governance** vào `CONTROL_PLANE_GROUPS` chứa **Policies**. Giữ **Data Lineage** ở Explore (Placeholder cho phase 2).
- **Route** (`ControlPlaneRoutes`, `RequireGlobalAdmin`): `policies`, `policies/new`, `policies/:id`.
- **API**: `frontend/src/api/policies.ts` + hooks.
- **Trang/Component** (shadcn + OKLCH; soi Ranger admin console):
  - `PoliciesList` — profile selector + tab loại (Access/Masking/Row-filter); bảng Name·Resource·Principals·Permissions·Enabled·Actions.
  - `PolicyEditor` — form render theo service-def: resource picker (catalog→schema→table→column, wildcard/exclude/recursive); Access (principal + access-types + Allow/Deny/Exclude); Masking (mask-type theo cột); Row-filter (biểu thức theo nhóm principal).
  - **Principal picker**: combobox từ `/control-plane/principals`.

## 4. Lớp B — artifact config enforce cho Trino
Mỗi profile có **bundle config tải về** (+ hướng dẫn cài tay) — **không đổi image** vì plugin native:
- `access-control.properties`: `access-control.name=ranger`, `ranger.service.name={service}`, `ranger.plugin.config.resource=ranger-trino-security.xml`.
- `ranger-trino-security.xml`: `ranger.plugin.trino.policy.rest.url={egret base}`, `ranger.plugin.trino.service.name`, interval tải, wiring auth token nếu có.
- Nút **"Download Trino config"** ở trang profile.
- Nhét tự động vào coordinator lúc provision là **follow-up nhỏ**, ngoài spec này (ghi rõ để không ngầm coi đã xong).

## 5. Kiểm thử
- **Unit**: round-trip `ranger_json` cả 3 loại; map resource/exclude/wildcard; principal.
- **Route (DB)**: create→list(lọc)→get→update→delete; `policy_version` tăng mỗi lần ghi; cô lập theo service; `require_platform_admin` chặn non-admin; auto-bind tạo+link service khi provision.
- **Enforce (harness Java)**: mở rộng `tests/ranger/EgretRangerEnforcementTest` cho masking + row-filter tạo qua API mới — đây là **manual Trino integration test** chạy trước khi push.
- **Frontend (Vitest)**: serialize DTO editor; `PoliciesList` render + tabs + empty; selector đổi service.
- **Trước merge**: full local CI (fmt/clippy/test + frontend lint/typecheck/test/build); smoke create→download config→enforce.

## 6. Phân pha
- **P1** — chuyển menu; migration + auto-bind; backend `services`+`service-def`+`list/get`; `PoliciesList` read-only + selector + tabs.
- **P2** — Access CRUD đầy đủ; picker principal/resource; `ranger_json` create/update.
- **P3** — Masking + Row-filter editor; bundle Lớp B + test enforce harness.

## 7. Rủi ro
- **Độ trung thực DTO↔Ranger JSON** (masking/row-filter) — phủ round-trip test trước khi nối UI.
- **Migration staleness**: chạm `metastore/src/lib.rs`.
- **Bind idempotent / share / xóa**: không tạo trùng service; `link` validate tồn tại + khớp type; xóa engine phải **giữ** profile share/standalone (`ON DELETE SET NULL`, không cascade service).
- **Dev Trino file-ACL mặc định**: verify enforce qua harness `tests/ranger`, không phải docker mặc định — nói đúng cái đã chạy.
- **Tên principal** phải khớp group-mapping của Trino, nếu không Allow/Deny không match user runtime.

## 8. Ngoài scope (sau)
- **Data Lineage** (OpenLineage + graph UI, tham chiếu Marquez) — sub-epic 2.
- **StarRocks** def + provisioning + enforce.
- **Tag-based policy** và **access-audit** view.
- Nhét config Trino **tự động** lúc provision.
- **Workspace-scoped** policy view (epic này global/admin).
