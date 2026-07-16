# StarRocks ↔ Ranger — Cưỡng chế chính sách (Full Native Parity) — Thiết kế

**Ngày:** 2026-07-16 · **Trạng thái:** Thiết kế — chờ duyệt · **Nhánh:** `feat/starrocks-ranger-enforcement` (off main)
**Chương trình:** StarRocks ngang-hàng-với-Trino, **lát 1/3** (hai lát còn lại — cấp phát engine trên control-plane, và delegation/bảo mật — là các spec riêng sau này). Spec này chỉ bao **cưỡng chế chính sách Ranger cho StarRocks**.

> Tài liệu tiếng Anh là bản chuẩn: `docs/en/specs/2026-07-16-starrocks-ranger-enforcement-design.md`. Bản tiếng Việt này là bản tham chiếu.

## Mục tiêu

Đưa StarRocks lên **tất cả những gì StarRocks + Apache Ranger hỗ trợ nguyên bản** cho quản trị dữ liệu, ngang hàng với cưỡng chế Ranger của Trino trong egret:

- **Chính sách truy cập theo tài nguyên (resource-based)** trên toàn bộ cây tài nguyên StarRocks (catalog → database → table → column, cùng view / materialized_view / function / global_function / resource / resource_group / storage_volume / user / system).
- **Chính sách che cột (column-masking)** (6 kiểu mask nguyên bản của StarRocks).
- **Chính sách lọc theo hàng (row-level-filter)**.

Chính sách được **soạn trong egret**, **phục vụ bởi REST `/service` tương thích Ranger-Admin của egret**, và **cưỡng chế bởi tích hợp Ranger tích-hợp-sẵn trong StarRocks FE**. Kết quả được chứng minh trực tiếp (live): một cột bị che vs. rõ, một tập kết quả bị lọc hàng, và một `SELECT` bị từ chối — mỗi cái khác nhau giữa principal có-quyền và không-quyền trên truy vấn StarRocks thật.

## Ngoài phạm vi (ranh giới rõ ràng)

- **Chính sách theo TAG: KHÔNG.** Plugin Ranger của StarRocks chỉ hỗ trợ **chính sách theo tài nguyên** — chính sách theo tag (access/mask/row-filter) là feature request thượng nguồn còn mở ([StarRocks #67458](https://github.com/StarRocks/starrocks/issues/67458)), chưa hiện thực. StarRocks FE **sẽ không bao giờ poll tag service** (`/service/tags/download/...`), nên mô hình tag-phân-loại → mask (P3.2) của egret **không** chảy sang StarRocks nguyên bản. Đây là khoảng trống có tài liệu, vĩnh viễn cho tới khi thượng nguồn làm — không phải bug. (Gắn tag một cột chỉ che trên Trino; che trên StarRocks được soạn theo từng cột/tài nguyên.)
- **Cấp phát engine: KHÔNG** (lát riêng). egret không cấp phát StarRocks, nên cấu hình plugin Ranger được giao ở dạng **cấu hình lúc deploy** trên StarRocks dev, không do orchestrator sinh ra. Khi lát cấp-phát tới, nó sẽ chuyển cấu hình này vào StarRocks provisioner (tương tự P3.2 làm cho Trino).
- **Autocomplete của Ranger: KHÔNG.** service-def dùng `implClass: ""`; egret không hiện thực endpoint tra-cứu-tài-nguyên của Ranger. Soạn thảo dùng autocomplete catalog/identity của egret.
- **Đường truy vấn không đổi.** Delegation JWT theo từng user đã có sẵn (`StarRocksConnector` trình JWT engine OIDC qua plugin `mysql_clear_password`, login user = `preferred_username`). StarRocks FE ủy quyền theo principal đó; không cần đổi connector.

## Bối cảnh — hiện trạng (đã kiểm chứng trong mã nguồn)

- **Lớp phục vụ Ranger của egret đã bất-khả-tri-engine.** `ranger/routes.rs` gắn `ranger_router()` tại `/service` với `download_policies` + `download_tags`, nhận `{service}` bất kỳ và phục vụ service-def + policy đã lưu **nguyên văn**. Không rẽ nhánh theo engine — service StarRocks "cắm vào" phía phục vụ mà không đổi route.
- **Kho chính sách đa-service.** `PolicyStore` mô hình hóa toàn bộ bề mặt Ranger; mỗi lần ghi policy tăng `policy_version` để engine đang poll tự tải lại.
- **service-def được nhúng + seed lúc khởi động.** `service_def.rs` nhúng `trino.json` (id 203) + `tag.json` (id 204), seed từ `egret-bin/src/main.rs:975` khi `ranger.enabled`. **Chưa có def StarRocks.**
- **Soạn thảo còn hard-code Trino — đây là phần việc thật.** `admin/ranger_json.rs` đóng cứng `serviceType: "trino"`. Làm cho soạn thảo **nhận biết service-type** là hạng mục thật.
- **Auth trên `/service` ẩn danh khi không đặt token** (mặc định) — như smoke P3.2 của Trino. Bề mặt chỉ-đọc.
- **StarRocks dev đã có nhưng chưa có cấu hình Ranger.** `docker-compose.dev.yml` chạy `starrocks/allin1-ubuntu:latest`; `deploy/starrocks/` có artifact JWT nhưng chưa có cấu hình plugin Ranger. Hỗ trợ Ranger của StarRocks **tích hợp sẵn trong FE**, bật bằng cấu hình — **không cần cài jar**.

## Mô hình Ranger StarRocks (từ service-def chuẩn)

Nguồn: `conf/ranger/ranger-servicedef-starrocks.json`.

- **name:** `starrocks`
- **resources (name | parent | level):** `catalog|—|10`, `database|catalog|20`, `table|database|30`, `column|table|40`, `view|database|30`, `materialized_view|database|30`, `function|database|30`, `global_function|—|10`, `resource|—|10`, `resource_group|—|10`, `storage_volume|—|10`, `user|—|10`, `system|—|10`.
- **accessTypes:** `grant, node, operate, delete, drop, insert, select, alter, export, update, usage, plugin, file, blacklist, repository, refresh, impersonate, create database/table/view/function/global function/materialized view/resource/resource group/external catalog/storage volume`.
- **dataMaskDef.maskTypes:** `MASK, MASK_HASH, MASK_NULL, MASK_NONE, MASK_DATE_SHOW_YEAR, CUSTOM` (khác bộ của Trino).
- **rowFilterDef:** có. **policyConditions:** `ip-range`. **contextEnrichers:** không.

## Kiến trúc — ba đường nối

### 1. service-def + seed (`egret-policy`) — nhỏ
- Nhúng `service-defs/starrocks.json`: def chuẩn với **id 205** (Trino 203, tag 204), `implClass: ""`.
- Thêm `STARROCKS_DEF_NAME = "starrocks"`, `starrocks_service_def()`, `seed_starrocks(store)`; nối vào `main.rs:975` cạnh `seed_trino`/`seed_tag`, gate theo `ranger.enabled`.
- Unit test song song với test của Trino.

### 2. Soạn thảo nhận-biết-service-type (`admin/ranger_json.rs` + FE) — vừa
- **Backend:** luồng service-type của profile mục tiêu qua serializer để body mang `serviceType: "starrocks"` cho profile StarRocks; đường Trino không đổi. Thêm test.
- **Frontend:** lấy **danh sách mask-type** + **cây tài nguyên** từ service-def của profile đang chọn (thay vì hằng số Trino).
- **Tạo profile:** cho phép tạo profile kiểu `starrocks` (`create_service(name, "starrocks", cfg)`) từ UI Policies — tương đương thủ công của auto-bind khi cấp phát Trino.

### 3. Cấu hình deploy + live smoke (`deploy/starrocks`) — vừa, chỉ-live
- `ranger-starrocks-security.xml` (`service.name=<profile>`, `policy.rest.url=<egret base>`, `RangerAdminRESTClient`, `pollIntervalMs`, cache dir), `ranger-starrocks-audit.xml` tối thiểu, `fe.conf: access_control=ranger`, mount vào FE `allin1-ubuntu` qua compose, cập nhật README.
- **Live smoke:** tạo bảng thật có cột nhạy cảm; soạn 3 policy (mask `MASK_HASH`, row-filter, access-deny); xác nhận FE poll `/service/plugins/policies/download/<profile>`; truy vấn 2 principal, ghi lại che-vs-rõ, lọc-vs-đầy-đủ, cho-phép-vs-từ-chối. Ba cái phân kỳ = **CƯỠNG CHẾ RANGER STARROCKS ĐÃ KIỂM CHỨNG LIVE**. Báo cáo đúng những gì đã chạy (không phải test tự động — bề mặt này CHỈ-LIVE, như P3.2 của Trino).

## Luồng dữ liệu

Seed def `starrocks` lúc khởi động → admin tạo profile `starrocks` + soạn policy → mỗi lần ghi tăng `policy_version` → StarRocks FE poll & cache ServicePolicies → user truy vấn qua `StarRocksConnector` (JWT của họ) → StarRocks FE cưỡng chế deny/mask/filter theo principal.

## Rủi ro chỉ-live (kiểm ở smoke, như P3.2)

1. FE `allin1-ubuntu` bật `access_control=ranger` chỉ-bằng-cấu-hình (không cài jar).
2. Tên chính xác các thuộc tính `ranger.plugin.starrocks.*` — lệch thì sửa 1 dòng XML.
3. Download ẩn danh được `RangerAdminRESTClient` của StarRocks chấp nhận (như Trino).

## Phân rã công việc (cho plan)

1. service-def + seed. 2. Serializer nhận-biết-service-type. 3. FE soạn thảo theo-def + tạo profile `starrocks`. 4. Cấu hình deploy. 5. Cổng full-workspace + LIVE SMOKE, báo cáo đúng những gì đã chạy.
