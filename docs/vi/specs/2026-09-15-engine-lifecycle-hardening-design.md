# SP1 — Engine lifecycle hardening

**Trạng thái:** thiết kế, chờ review
**Chương trình:** `docs/{en,vi}/specs/2026-09-15-control-plane-dg-review-programme.md`
**Triển khai:** hai PR — Part A (delete/unassign, đúng bug của user) rồi đến Part B (hardening provisioning)

> Bản tiếng Anh là bản chuẩn:
> `docs/en/specs/2026-09-15-engine-lifecycle-hardening-design.md`.

## 1. Vấn đề

Xoá một engine trả về "internal error" mà không có gì trong log, và lúc đó các object Kubernetes của engine đã biến mất trong khi row của nó vẫn còn. Review lại đường xoá và phần còn lại của provisioning tìm ra nguyên nhân cùng một loạt defect về thứ tự, dọn dẹp và secret.

## 2. Các phát hiện

Handler nằm trong `crates/egret-engine-mgmt/src/engines.rs` trừ khi có ghi chú khác. **V** = đã xác minh trong source khi viết spec này; phần còn lại đến từ review và cần được xác minh lại khi lập kế hoạch.

### 2.1 Đường xoá (delete path)

Thứ tự hiện tại (`engines.rs:3884-3894`): xoá Kubernetes → `DELETE FROM engine_mgmt.engines` → thu hồi catalog client (lỗi bị bỏ qua). Việc xoá row cascades sang `engine_catalogs`, `engine_workspaces` và các row `query.databases` được materialize từ engine, và từ đó cascades tiếp.

| # | Defect | |
|---|---|---|
| D1 | `dataapi.endpoints.database_id … ON DELETE RESTRICT` (`migrations/0121_dataapi_endpoints.sql:30`). Bất kỳ endpoint nào trên một database đã materialize — **kể cả đã bị trash** (0126 là soft delete) — đều khiến DELETE thất bại. Trên cluster kind, `demo-trino` có 26 endpoint như vậy. | V |
| D2 | `automation.schedules.database_id … ON DELETE SET NULL` (`0110:24`) xung đột với `CHECK schedule_alert_needs_query` (`0110:71`, alert bắt buộc phải có `database_id`). Bất kỳ SQL alert nào cũng chặn việc xoá. Comment trong `0066_query_schema.sql` khẳng định việc xoá không thể bị chặn là đã lỗi thời (stale). | V |
| D3 | `EngineMgmtError::Db(_) => EngineHttpError::Internal` (`http_error.rs:61,72`) bỏ qua `sqlx::Error` mà không log lại. | V |
| D4 | Kubernetes bị xoá trước khi biết chắc DB delete có thành công hay không → cluster mất, row và các dependent còn lại. | V |
| D5 | StarRocks delete xoá CR bằng `?`; `KubeClient::delete` coi 404 là lỗi → mỗi lần retry sau khi CR đã mất đều trả về 400. | |
| D6 | Trino delete bọc mọi lời gọi trong `let _ =` → lỗi 403/timeout/API server không truy cập được đều bị bỏ qua, row bị xoá, cluster bị bỏ rơi (orphan). | |
| D7 | Không có Kubernetes được cấu hình → delete bỏ qua cluster và trả về 204 (trong khi create trả về 400). | V |
| D8 | Khi delete thành công, nó âm thầm cascade sang dataset → chart → dashboard card → schedule (và cả run). Dialog xác nhận chỉ nhắc đến tài nguyên Kubernetes. | |
| D9 | Không có gì được dọn dẹp: Ranger profile và `<name>_tag` service được tạo tự động, các secret (`delegator_secret_id`, `tls_secret_id`, `tls_cert_secret_ref`, `root_secret_id`), và việc thu hồi bỏ qua `is_minted_client`. Tạo lại một engine cùng tên sẽ gặp `AlreadyExists` trong `create_service`, chỉ log warning rồi tiếp tục **không được gắn (unbound) → Trino allow-all trong khi Ranger vẫn bật**. | |
| D10 | Bỏ gán (unassign) một engine khỏi workspace (`DELETE …/workspaces/{ws}`, `PUT /engines/{id}/workspaces`) sẽ xoá database đã materialize → cùng lỗi 500 D1/D2. `PUT` không nguyên tử (non-atomic) (có thể áp dụng phần assign rồi mới fail ở phần unassign). Gán vào một workspace không tồn tại trả về 500 thay vì 404. | |

**Sửa lại (review kế hoạch):** một cơ chế purge đã có sẵn — `DELETE /api/ws/{ws}/data-api/endpoints/{id}` hard-delete (`EndpointStore::delete`, `endpoints.rs:1215`) và trang trash đã có "Delete permanently". Kiểm tra "không có purge" trước đó đọc phải một kết quả grep bị cắt cụt. §3.3 được rút gọn thành một test pin lại những gì purge xoá.

### 2.2 Provisioning

| # | Defect | |
|---|---|---|
| P1 | **Bảo mật.** `TRINO_INTERNAL_SECRET = "egret-internal-{engine.id}"` (`engines.rs:659`) và mật khẩu monitor `egret-monitor-{id}` (`:1457`) có thể suy ra được từ engine UUID, thứ mà thành viên workspace có thể đọc được (`Database.engine_id`, `/api/ws/{ws}/engines/{id}`). Bất kỳ ai chạm được tới coordinator đều có thể giả mạo token giao tiếp nội bộ hoặc đăng nhập như `egret-monitor`. | V |
| P2 | Một lần create thất bại sẽ xoá row và thu hồi client, nhưng để lại các object Kubernetes đã áp dụng, các row secret TLS/delegator và Ranger policy đã seed. | |
| P3 | Trino update lưu spec rồi mới trả về 400 nếu Kubernetes lỗi — DB đi trước cluster, không có rollback. Một body không có `delegationMode` sẽ âm thầm đọc lại thành `impersonate`. | |
| P4 | `rematerialize_for_engine` chỉ chạy cho StarRocks; một thay đổi delegation mode hoặc TLS trên Trino để lại các workspace connection bị stale. | |
| P5 | `remove_catalog` xoá row và thu hồi credential trước khi update Kubernetes; nếu thất bại, cluster vẫn còn một catalog với credential đã bị thu hồi và lần retry sau không còn gì để tác động. | |
| P6 | `engines.status` không bao giờ được cập nhật (`set_engine_status` không có caller nào); mọi engine đều đọc ra `provisioning` bất cứ khi nào live status không có sẵn. | |
| P7 | Background task bootstrap StarRocks không kiểm tra xem engine có còn tồn tại hay không. | |
| P8 | `redact_catalog_config` chỉ redact credential cho `egret-warehouse`; các property nhạy cảm inline của loại khác vẫn được trả về cho `engine:read`. | |
| C1 | `engine_catalogs.kind` vẫn còn `CHECK (kind IN ('egret-warehouse','postgresql','tpch','tpcds','generic'))` (`0057:10`, chưa bao giờ bị drop), trong khi `add_catalog` v2 "connector-first" và UI lại cho phép mysql, iceberg, clickhouse, … → lỗi 500. | V |

## 3. Thiết kế — Part A: delete và unassign

### 3.1 Dependent

Một **dependent** là một asset do người dùng tạo ra mà việc xoá sẽ phá huỷ, hoặc thứ đang chặn việc xoá. Được tính trên các row `query.databases` có `engine_id = <engine>` (có thể giới hạn theo một workspace):

| Kind | Nguồn | Chặn |
|---|---|---|
| Data API endpoint (live, draft, disabled) | `dataapi.endpoints` | có |
| Data API endpoint trong thùng rác | `dataapi.endpoints WHERE deleted_at IS NOT NULL` | có (báo cáo riêng, purge được, xem §3.3) |
| Schedule (alert, report) trên database | `automation.schedules.database_id` | có |
| Dataset (và qua đó, chart, dashboard card, report schedule) | `analytics.datasets` | có |
| Lịch sử query | `query.queries` | không — cascade |
| Notebook engine access | `notebook_engine_access` | không — cascade |
| Saved query | `saved_queries` | không — SET NULL (đã có sẵn) |

Planner phải liệt kê lại cây FK còn sống (`pg_constraint`) và fail một test nếu xuất hiện một con RESTRICT mới, hoặc một con NOT-NULL-vs-SET-NULL mới của `query.databases` mà preflight chưa bao phủ.

### 3.2 `DELETE /api/control-plane/engines/{id}`

1. `authz::require("engine:delete")`; load engine (404).
2. **Preflight.** Tính các dependent. Nếu có bất kỳ loại chặn nào khác 0 → **409**:
   ```json
   {"code":"engine_has_dependents",
    "dependents":[{"workspaceId":"…","workspaceName":"…","kind":"dataapi_endpoint","count":25,"sample":["orders-by-day", "…"]},
                  {"workspaceId":"…","workspaceName":"…","kind":"dataapi_endpoint_trashed","count":1,"sample":["…"]}]}
   ```
   (`sample` ≤ 5 tên.) Không có gì bị thay đổi.
3. Kubernetes phải được cấu hình; nếu không **400** "kubernetes not configured" (giống như create).
4. Set `status = 'deleting'`.
5. **Xoá Kubernetes**, theo từng object. `KubeClient::delete` trả về `Ok` khi gặp 404. Bất kỳ lỗi nào khác →
   **502** `engine_cluster_delete_failed` kèm nguyên nhân; row giữ nguyên `deleting` và request có thể retry. Cả Trino và StarRocks đều propagate lỗi khác 404.
6. **Một transaction DB duy nhất:** chạy lại preflight chặn bên trong transaction (race với một endpoint được tạo đồng thời → 409, row giữ `deleting`, cluster đã mất — chấp nhận được, lần retry tiếp theo sẽ thành công một khi dependent đã được xử lý); xoá row engine (cascade như hiện tại); xoá các row secret của engine (`delegator_secret_id`, `root_secret_id`, `tls_secret_id` — không bao giờ xoá `tls_cert_secret_ref` do admin cung cấp). **Ranger profile và tag service không bao giờ bị xoá** (quyết định của user ngày 2026-09-15): việc xoá row chỉ gỡ gắn (unbind); Part B tái sử dụng một profile mồ côi khi tạo lại.
7. Sau khi commit: thu hồi catalog client nơi `is_minted_client`; lỗi chỉ được log, không được trả về.
8. **204.**

Backstop: `From<EngineMgmtError>` / `From<CpError>` log `sqlx::Error` ở mức `error`, và map SQLSTATE `23503` (FK) và `23514` (CHECK) thành **409** `constraint_violation` nêu tên constraint.

### 3.3 Purge các Data API endpoint đã bị trash

Route và UI purge đã có sẵn (xem phần sửa lại ở §2.1). Part A chỉ thêm một test pin lại việc purge xoá các version và schedule `dataapi_alert` của endpoint, đồng thời giữ nguyên các row `access_log` (không có FK, theo thiết kế).

### 3.4 Unassign

`DELETE /api/admin/workspaces/{ws}/engines/{id}` và `PUT /api/control-plane/engines/{id}/workspaces` chạy preflight giới hạn trong (các) workspace bị ảnh hưởng **trước khi ghi bất cứ gì**. `PUT` tính diff đầy đủ, preflight mọi lượt unassign, rồi áp dụng assign và unassign trong một transaction. Workspace không tồn tại → 404.

### 3.5 UI

- Dialog xoá của `EngineDetail` nêu rõ phạm vi: cluster bị huỷ; lịch sử query và notebook access trên các workspace connection của engine này bị xoá; saved query mất kết nối. Xác nhận gõ đúng tên.
- Khi gặp 409, dialog hiển thị bảng dependent (workspace, kind, count, sample) kèm link tới trang Data API / alert / dataset của từng workspace. Không có nút xoá cho đến khi danh sách rỗng.
- Khi gặp 502, hiển thị toast kèm nguyên nhân; `status = deleting` hiển thị badge "Deleting — retry" với nút **Retry delete**.
- Tab gán workspace hiển thị cùng bảng dependent khi gặp 409.

## 4. Thiết kế — Part B: hardening provisioning

1. **Secret nội bộ ngẫu nhiên (P1).** Hai cột nullable mới `engines.internal_secret_id`, `engines.monitor_secret_id` → secret toàn cục được đặt tên, chứa 32 byte ngẫu nhiên (base64). Được tạo khi provision; với engine đã tồn tại, chúng được tạo ở lần apply kế tiếp (update, đổi catalog, restart) — giá trị derive không bao giờ được dùng lại một khi đã có row secret. Route mới `POST /api/control-plane/engines/{id}/rotate-secrets` (`engine:update`) tạo lại cả hai và apply lại (pod restart). EngineDetail → Security & Exposure có thêm nút **Rotate secrets** với xác nhận "pods sẽ restart". Secret bị xoá cùng engine (§3.2 bước 6).
2. **Tạo lại cùng tên (D9).** `create_service` gặp `AlreadyExists` → nếu profile hiện có không gắn với engine nào thì tái sử dụng nó (và tag service của nó); nếu đã gắn thì create thất bại với 409. Khi `config.ranger.enabled`, một engine Trino không bao giờ được provision ở trạng thái unbound.
3. **Bù trừ khi create thất bại (P2).** Việc bù trừ (compensation) xoá các object Kubernetes đã áp dụng (chấp nhận 404), các row secret TLS/delegator/internal/monitor, và các policy đã seed cho engine này, bên cạnh row và client. Mỗi bước tự log lỗi riêng và tiếp tục.
4. **Update (P3).** Snapshot spec đang lưu; lưu spec mới; nếu Kubernetes lỗi thì khôi phục snapshot và trả về 502 (giống `update_catalog`). Thiếu `delegationMode` thì giữ nguyên giá trị đã lưu.
5. **Rematerialize cho Trino (P4).** Bất kỳ thay đổi nào về delegation mode hoặc TLS đều gọi `rematerialize_for_engine` cho Trino giống như cho StarRocks.
6. **`remove_catalog` (P5).** Update Kubernetes với catalog đã bị xoá trước; nếu thành công mới xoá row, sau đó thu hồi client nếu là minted.
7. **Status (P6).** `status ∈ {provisioning, running, failed, deleting}` với một CHECK (migration map trước mọi giá trị khác về `provisioning`). `create` set `provisioning`; một lần provision thất bại chưa được rollback set `failed`; `get_engine` ghi `running` khi live status báo mọi pod đã ready và giá trị đang lưu là `provisioning`; delete set `deleting`. UI hiển thị live status khi có, nếu không thì hiển thị giá trị đã lưu.
8. **StarRocks bootstrap (P7).** Background task đọc lại engine theo id và `name` trước mỗi lần ghi và thoát nếu engine đã mất hoặc đã được tạo lại.
9. **Redaction (P8).** `redact_catalog_config` redact, cho mọi kind, `credential`, `clientRowId` và bất kỳ property nào có key khớp `(?i)(password|secret|token|credential|private[-_.]?key|access[-_.]?key)`.
10. **Catalog kind (C1).** Migration drop CHECK `kind` trên `engine_mgmt.engine_catalogs`; việc validate chuyển vào `add_catalog`.

## 5. Testing

- **Part A, DB route test** (`crates/egret-engine-mgmt/tests/routes.rs`), mỗi loại chặn một test: endpoint, endpoint đã trash, SQL alert, dataset → 409 đúng `kind`/`count`, row engine và FakeKube không đổi. Happy path → 204, secret và profile tự động biến mất, minted client bị thu hồi. Unassign với mỗi loại chặn → 409, không row gán nào thay đổi; `PUT` trộn assign/unassign với một cái bị chặn → không gì được áp dụng.
- **Test bảo vệ cây FK**: query `pg_constraint` để lấy các con của `query.databases`; assert tập (table, on_delete) bằng đúng tập mà preflight phân loại.
- **FakeKube**: delete trả 404 → thành công; trả 500 → 502 và `status = deleting`; retry sau khi sửa → 204.
- **Error mapping**: cố tình gây vi phạm FK → 409 `constraint_violation`, và log chứa SQLSTATE.
- **Purge** (`crates/egret-dataapi/tests/trash.rs`): purge một endpoint đã trash sẽ xoá các version và schedule `dataapi_alert` của nó, đồng thời giữ nguyên các row `access_log`. Xoá engine giữ nguyên Ranger profile (pin lại).
- **Part B**: tạo lại cùng tên gắn vào profile mồ côi (regression cho allow-all); rotate-secrets đổi env secret đã render và hash của password.db; update thất bại khôi phục spec; `remove_catalog` thất bại giữ nguyên row và client; redaction cho catalog `postgresql` có mật khẩu inline; thêm catalog `mysql` thành công sau migration.
- **Frontend**: render bảng dependent từ fixture 409; badge retry-delete.
- **Live trên kind**: đếm dependent của `demo-trino` theo kiểu read-only trước; sau đó `DELETE` → 409 liệt kê chúng (không phải 500). Không có dữ liệu nào trên cluster bị thay đổi mà không có quyết định của người dùng.

## 6. Ngoài phạm vi

- Chuyển endpoint/dataset/alert sang connection khác ("re-point") — một tiện ích cho sau này.
- Dọn 18 Kubernetes Secret `*-tls` mồ côi đã có sẵn trong `egret-engines` (thủ công, tuỳ người dùng quyết định).
- Bất cứ điều gì thuộc SP2–SP5.
