# Control Plane & Data Governance — các phát hiện review và chương trình

**Trạng thái:** review hoàn tất; SP1 và SP2 đã được thiết kế, SP3–SP5 chưa được thiết kế
**Ngày:** 2026-09-15
**Các spec trong chương trình này:**
- SP1 `docs/{en,vi}/specs/2026-09-15-engine-lifecycle-hardening-design.md`
- SP2 `docs/{en,vi}/specs/2026-09-15-governance-app-split-design.md`

> Bản tiếng Anh là bản chuẩn:
> `docs/en/specs/2026-09-15-control-plane-dg-review-programme.md`.

## 1. Người dùng yêu cầu gì

1. Audit log không phải là log truy cập/từ chối dữ liệu; đó là log toàn hệ thống → chuyển ra khỏi Data Governance.
2. Xoá một engine trả về "internal error" → review kỹ toàn bộ engine provisioning.
3. Policy và tag của DG là hai hệ thống policy riêng biệt, gây nhầm lẫn → nghiên cứu và thiết kế lại policy toàn bộ.
4. Data Governance trở thành menu riêng, ngang hàng với Control Plane; một người dùng DG không cần phải là system admin.
5. Infra có thêm **Catalogs**: cấu hình kết nối theo từng loại catalog (database, iceberg, …); warehouse chuyển vào đây (≈ iceberg catalog); catalog là thứ mà Trino/StarRocks provisioning gắn vào.
6. Trên nền catalog, một mô hình phân quyền dữ liệu kiểu egret-native:
   1. giữ nguyên Ranger policy như hiện tại, cho Trino/StarRocks bên ngoài;
   2. một mô hình egret (user/group × đối tượng catalog × select/insert/delete/…) sinh ra các profile Ranger tương ứng, để engine bên ngoài có thể tích hợp; engine do egret provision được gắn với profile mặc định này;
   3. mở rộng sang engine không có Ranger (vd: Postgres): egret kiểm tra quyền của user, chạy query bằng credential của catalog hoặc chặn lại; row filter / masking bằng cách viết lại SQL hoặc xử lý hậu kỳ kết quả.

## 2. Các phát hiện

### 2.1 Audit log (mục 1)

`audit.audit_log` (migration 0076) ghi lại hai loại: **decision** — mọi lời gọi Cedar PDP ở tầng API (`egret-auth/src/pdp/audit.rs`), và **mutation** — role/grant, gán tag, thành viên workspace, Data API, workflow và các hành động impersonation. Nó **không** ghi lại việc truy cập dữ liệu của engine (Ranger audit đang tắt: `xasecure.audit.is.enabled=false`), đăng nhập, hay thay đổi cấu hình engine, warehouse, storage, secret hoặc policy. Cách hiểu của người dùng là đúng: đây là log hệ thống. Được gate bởi `audit:read`, quyền này không thể grant (chỉ platform_admin).

### 2.2 Navigation và quyền (mục 4)

- Chế độ sidebar được chọn hoàn toàn dựa trên tiền tố URL `/control-plane` (`Sidebar.tsx:126`).
- Đã có sẵn role `data_steward` ở cấp platform (policy:*, tag:*, certify, read trên infra/identity).
- `tag:read` bị thiếu ở cả hai bản sao của `CONTROL_PLANE_PERMS` (`Header.tsx`, `guards.tsx`): một user chỉ có quyền tag thì không thể vào được luôn.

### 2.3 Policy và tag là ba hệ thống, không phải hai (mục 3)

| Hệ thống | Nơi lưu | Được thực thi bởi | UI |
|---|---|---|---|
| Ranger resource policy (access / masking / row filter) | `policy.authz_services` (profile) + `authz_policies` | Ranger plugin bên trong Trino/StarRocks, poll egret `/service` | Policies → profile → các tab |
| Ranger tag policy | tag service `<engine>_tag`, tag từ `catalog_tag_assignments` | Chỉ Trino plugin (StarRocks plugin không có tag) | Policies → profile → tab Tags; gán tag ở WarehouseDetail |
| Cedar classification clearance | `classification_policies`, `tag_assignments` (đối tượng BI) | Tầng API của egret | Classifications → tag detail |

Hệ thống 2 và 3 dùng chung bảng `tags` và chung trang Classifications nhưng ngữ nghĩa không liên quan gì với nhau: clearance ẩn toàn bộ đối tượng BI, nó không thể mask một cột catalog. Các defect tìm được (sửa trong SP4, không sớm hơn):
- **Sửa một tag policy luôn thất bại**: `update_policy` (`egret-policy/src/admin/mod.rs:1040-1060`) không rẽ nhánh theo loại tag, trong khi create thì có.
- Tag policy lưu `tags.name` — vốn có thể đổi và không unique; Cedar lại dùng khoá `tags.key`. Đổi tên tag sẽ âm thầm phá vỡ Ranger tag policy.
- Principal đôi khi được ghi dưới dạng email (policy picker), đôi khi dưới dạng username (auto-admin policy). *Suy ra, cần xác minh:* không có group provider hay usersync nào tồn tại, nên rule theo group có thể không bao giờ khớp bên trong engine.
- *Suy ra, cần xác minh:* một engine được gắn vào một profile đã tồn tại nhưng tên khác thì poll một service mang tên theo engine.

### 2.4 Engine provisioning (mục 2)

Nguyên nhân gốc của lỗi 500 mà user gặp phải, cùng mười lăm defect khác nữa — xem SP1 §2.

### 2.5 Catalogs (mục 5)

- **Chưa có catalog registry.** Mỗi catalog là một row `engine_mgmt.engine_catalogs` thuộc về một engine; cùng một kết nối Postgres trên hai engine là hai row với hai bộ secret khác nhau.
- StarRocks bỏ qua mọi catalog row không phải primary (chỉ catalog `egret` primary được một background task nối dây).
- Warehouse thuộc về một workspace duy nhất (`catalog.warehouses.workspace_id NOT NULL`).
- Storage connection là registry kết nối toàn cục, độc lập với engine, duy nhất tồn tại hiện nay — đây là pattern cần theo.
- Mô hình Polaris đã bị gỡ bỏ (0051, drop ở 0054) có một kết nối toàn cục + gán theo workspace: tiền lệ gần nhất.

### 2.6 Khả năng thực thi (mục 6)

- Workspace connection (`query.databases.engine`) chỉ hỗ trợ `trino` và `starrocks`; **không có** connector query cho Postgres/MySQL/DuckDB.
- **Không có SQL parser** (không có sqlparser-rs), không có table-reference extraction, không có rewriter; chỉ có một scanner viết tay cho Data API.
- **Không có Rust policy evaluator**; egret chỉ lưu và phục vụ Ranger JSON, engine tự đánh giá.
- Result set không mang column lineage (`TabularResult{columns{name,ty},rows}`).
- Mọi đường thực thi phía server đều hội tụ về `build_connector` (`egret-query/src/execution/connector/mod.rs:140`) qua ba kiểu entry: `run_query` (SQL Lab, BI, alerts), `QueryExecPort` (Data API, workflows — có nhận biết principal), và gọi trực tiếp (schema browse, introspection, tester). Chỉ `QueryExecPort` mang principal thật.
- **Notebook bỏ qua egret**: kernel nhận một engine JWT theo từng user và kết nối trực tiếp; Iceberg read dùng `AllowAllAuthorizer`.

## 3. Chương trình

| SP | Phạm vi | Phụ thuộc | Trạng thái |
|---|---|---|---|
| **SP1** | Engine lifecycle hardening: preflight delete/unassign, thứ tự k8s, error logging, dọn dẹp, derived secret, thứ tự update/catalog, status, CHECK cho catalog `kind` | — | đã thiết kế |
| **SP2** | Governance app split: `/governance/*` ngang hàng Control Plane; audit log → Control Plane → System | — | đã thiết kế |
| **SP3** | Catalog registry dưới Infra: kết nối có kiểu (database, iceberg, …), warehouse trở thành iceberg catalog, engine gắn vào catalog registry (Trino và StarRocks) | — | chưa thiết kế |
| **SP4** | Một mô hình phân quyền dữ liệu egret trên nền registry catalog, thay thế ba hệ thống ở §2.3; compile ra Ranger profile; engine do egret provision gắn với profile mặc định; các Ranger profile viết tay được giữ lại cho engine bên ngoài | SP3 | chưa thiết kế |
| **SP5** | Engine do egret thực thi (Postgres trước tiên): connector, SQL parser, policy evaluator, viết lại filter/mask, truyền principal qua mọi đường thực thi | SP4 | chưa thiết kế |

SP1 và SP2 độc lập với nhau và với SP3–SP5.

## 4. Các lập trường đã chốt cho SP3–SP5 (cần xác nhận lại khi thiết kế chúng)

- **Masking và row filter phải được áp dụng bằng cách viết lại SQL, không phải bằng xử lý hậu kỳ kết quả.** Xử lý hậu kỳ chỉ mask phần projection: `WHERE email = 'x'`, `GROUP BY email`, join và `count(DISTINCT email)` vẫn đọc được giá trị thật và làm lộ nó. Ranger trong Trino cũng viết lại (nó bọc table reference). Xử lý hậu kỳ chỉ chấp nhận được như một lớp phòng thủ bổ sung.
- **Mô hình egret là nguồn sự thật; Ranger JSON là một artefact được compile ra.** Một profile được sinh ra là read-only trong trình soạn Ranger; các profile viết tay vẫn là một loại riêng.
- **Việc thực thi phía egret chỉ bao phủ các query đi qua egret.** Notebook cần một đường SQL được proxy (hoặc phải bị từ chối) trên các engine không có Ranger.
- Điểm chặn (chokepoint) thực thi là một decorator `Connector` được xây trong `build_connector`, thứ đòi hỏi trước tiên phải có một principal (user id, username, groups) ở mọi kiểu entry.
