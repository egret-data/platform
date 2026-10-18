# Engine-scoped grants (F11): thiết kế

> Bản tham khảo tiếng Việt. Bản tiếng Anh [docs/en/specs/2026-10-18-perm-engine-grants-design.md](../../en/specs/2026-10-18-perm-engine-grants-design.md) là bản chuẩn; hai bản lệch nhau thì theo bản tiếng Anh.

Ngày: 2026-10-18. Trạng thái: các quyết định ghi bên dưới; user đã chốt phạm vi (catalogue chọn lọc) ngày 2026-10-18; mọi
dòng khác ghi "quyết định mặc định" là khuyến nghị của tài liệu này. Gốc: main `4a67321b` (#692). Nhánh
`feat/perm-engine-grants`, worktree `egret-f11`. Chỉ backend.

Quyết định F11 của spec chương trình (`docs/en/specs/2026-10-16-dg-frontend-redesign-design.md`) biến quyền mức engine
thành grant thật trong mô hình của egret, do `egret-perm-compile` biên dịch, sau này được cấp từ tab "Engine permissions"
(dự án con #4, F12). Tài liệu này đặc tả gói backend mà #4 phụ thuộc vào.

**Nguồn của các khẳng định.** Mọi khẳng định về code egret đều kèm `file:line` tại `4a67321b`, đọc trong worktree này.
Ngữ nghĩa engine được đọc ở các bản checkout bên cạnh: Trino `data-portal/trino` tại `482-75-g280b81bbc4e` (egret chạy
Trino 483; `access-control.name=ranger` chọn plugin `trino-ranger` nằm trong cây mã Trino,
`crates/egret-engine-mgmt-api/src/ranger_config.rs:74`, `RangerSystemAccessControlFactory.java:30`); StarRocks
`data-portal/starrocks` main tại `148a7fdc44e` (egret chạy 4.1.6; phiên bản engine thực sự chạy được đo bằng
differential và lần kiểm sống, không phải bằng lần đọc này); Apache Ranger `data-portal/ranger`. Đường dẫn Trino dưới đây
nằm dưới `plugin/trino-ranger/src/main/java/io/trino/plugin/ranger/`; đường dẫn StarRocks nằm dưới
`fe/fe-core/src/main/java/com/starrocks/`.

## Đính chính brief

Brief dùng để viết spec này mang những tiền đề mà code hoặc engine phủ nhận. Mỗi tiền đề được đính chính ở đây và thiết kế
đi theo bản đính chính.

| # | brief nói | điều đúng | hệ quả |
|---|---|---|---|
| C1 | "xem/kill query của user khác" là `queryid` execute | Trino kiểm tra việc xem và kill query của user khác dưới dạng **`trinouser` = chủ query, `impersonate`** (`RangerSystemAccessControl.java:185-216`). `queryid` `execute` là `checkCanExecuteQuery` (`:177-182`), tức quyền chạy bất kỳ query nào, thứ egret đã cấp cho mọi người qua `egret-user-execute-query` (`crates/egret-engine-mgmt/src/delegator_policy.rs:90-99`) | không có capability `queryid`; "impersonate user X" là một capability có nhãn nói rõ nó cũng cho người được cấp xem và kill query của X (đó là cùng một quyền Ranger) |
| C2 | function execute là resource `function` | plugin kiểm tra `catalog`/`schema`/**`schemafunction`** với `execute` (`RangerSystemAccessControl.java:753-756`); resource `function` ở level 10 của service-def không bao giờ được đọc (`RangerTrinoResource.java:26-37` không khai báo khoá đó). `accessTypeRestrictions` của `schemafunction` không có `execute` (`crates/egret-policy/src/service-defs/trino.json`, giống hệt `ranger-servicedef-trino.json` của Ranger), nhưng restrictions chỉ định hình policy mặc định, không bao giờ ảnh hưởng việc đánh giá (`agents-common/.../RangerBaseService.java:195-213`) | capability biên dịch ra `schemafunction` + `execute`; test catalogue khẳng định đây là entry duy nhất nằm ngoài danh sách restriction của resource của nó |
| C3 | StarRocks "usage của một resource group" | không có quyền như vậy: resource group chỉ được kiểm tra cho `DROP` và `ALTER` (`sql/analyzer/AuthorizerStmtVisitor.java:920-940`); tạo mới là `system` `create resource group` (`:915`) | bỏ khỏi catalogue (E5) |
| C4 | StarRocks `system` `operate` cấp được | `OPERATE` chặn `ADMIN SET CONFIG`, `EXECUTE SCRIPT` (code chạy trên FE), fail point, `ADMIN SKIP COMMITTED TRANSACTION`, replica status (`AuthorizerStmtVisitor.java:1519-1524`, `:2300`, `:2315`, `:2343`, `:2449`) và việc kill connection của user khác (`qe/StmtExecutor.java:1878-1880`, `qe/ConnectScheduler.java:326-328`) | gần như root: không cấp được (E4); "kill query của user khác" không có capability chọn lọc nào trên StarRocks |
| C5 | nhiều engine có thể dùng chung một profile | một lần bind từ chối profile mà engine khác đang bind (I2, `crates/egret-engine-mgmt/src/engines.rs:5265-5281`, dùng chung cho create và đăng ký external). Không có unique index (`migrations/0058_engine_authz_service.sql:2-3`), nên các dòng cũ vẫn có thể dùng chung | danh sách reach (E9) được giữ, và là một engine trong mọi bản cài đặt tạo từ I2 trở đi |
| C6 | các đích bị từ chối `egret-lineage` / `egret_delegator` là "reserved" | chỉ `egret-lineage` nằm trong `RESERVED_ENGINE_USERNAMES` (`crates/egret-core/src/engine_principals.rs:16-19`); `egret_delegator` là một hằng trong `egret-engine-mgmt` (`delegator_policy.rs:15`), mà `egret-perm-compile` không thể phụ thuộc vào (nó là dev-dependency theo chiều ngược lại, `crates/egret-engine-mgmt/Cargo.toml:93`) | hằng được chuyển sang `egret_core::engine_principals` (re-export ở nơi đang dùng hôm nay); việc từ chối impersonation nêu tên cả hai. Thêm `egret_delegator` vào danh sách *username* reserved là một bản sửa riêng, không làm ở đây (E21) |
| C7 | DTO của People có thêm engine grant | route People trả về `standsFor` của principal; client liệt kê grant bằng cách so khớp với nó (P4, `crates/egret-perm-compile/src/people.rs:356-362`, `:473-521`). Không DTO nào mang grant | thay vào đó là một route liệt kê `GET /perm/engine-grants` (E15); không DTO People nào thay đổi |
| C8 | mở rộng `/perm/explain` với đích là capability | `ExplainRequest` bắt buộc `action` (`read`/`write`) và `target: Scope` (`crates/egret-perm-compile/src/routes.rs:460-464`); một câu hỏi về capability không có cái nào trong hai. Cho chúng thành tuỳ chọn sẽ đổi mọi caller được sinh ra | một route anh em `POST /perm/explain/capability` dùng lại cùng các mảnh theo từng profile (E13) |
| C9 | giá trị `{USER}` làm explain thành Undecidable | rộng hơn: trên mọi profile Trino có delegation, `egret-user-impersonate-self` được seed đặt `{USER}` ở level `trinouser` (`delegator_policy.rs:78-86`), và evaluator từ chối mọi request mang level đó (`crates/egret-ranger-eval/tests/eval.rs:1108-1135`) | mọi câu hỏi impersonation trên Trino đều Undecidable trên profile có delegation; explain nói rõ điều đó (E13). Dạy evaluator hiểu `{USER}` nằm ngoài phạm vi |
| C10 | sweep chỉ cần prefix mới | bốn chỗ chọn theo các prefix của compiler (E7), và `perm.compiled` không ghi được engine grant (`source_kind IN ('grant','rule')`, `migrations/0154_perm_model.sql:120-127`) | provenance là tên policy, như với clearance (`crates/egret-perm-compile/src/trigger.rs:1262-1276`) |
| C11 | (ngầm định) các đường recompile hiện có giữ profile của engine luôn cập nhật | rebind của một engine provisioned (`PUT /engines/{id}`) chỉ recompile profile MỚI (`engines.rs:5367-5440`, log "the previous profile row is untouched", `:5436`), và không đường xoá nào recompile (`engines.rs:6007-6009`). Update của một engine external thì đã recompile cả hai (`crates/egret-engine-mgmt/src/external.rs:406`, `:430-439`). Vô hại với grant gắn theo catalog; một grant gắn theo engine sẽ vẫn nằm biên dịch trong profile cũ | rebind của engine provisioned và cả hai đường xoá đều recompile profile trước đó (E10) |
| C12 | có thể đã có helper audit cho governance | `PermState` không mang audit sink nào (`routes.rs:103-112`), `egret-perm-compile` không phụ thuộc `egret-audit`, và `feat/dg-overview` (#3) không có commit nào sau `4a67321b` | một hook có tên ghi log hình dạng của sự kiện; #3 hoặc gói này, gói nào merge sau, sẽ nối `egret_audit::mutation_for` (E18) |

Đúng như đã nêu: số migration trên main dừng ở `0194` và `0195` còn trống; cổng chặn trùng version dựa theo tên file
(`crates/egret-metastore/src/lib.rs:173-221`); `apply_diff_in_tx` (`trigger.rs:975`) sweep theo prefix
(`trigger.rs:1016-1031`); `is_compiler_owned_policy_name` (`crates/egret-policy/src/store.rs:119-123`) và
`COMPILER_GRANT_PREFIX` (`store.rs:87`); trần 1000 policy mỗi profile (`trigger.rs:194`, thực thi ở `:1052-1066`);
`validate_user_ref` (`crates/egret-perm-compile/src/grants.rs:106-124`); `write_grant` (`routes.rs:1047`);
`request_resource` (`routes.rs:2834`); `egret_perm::eval::evaluate` chỉ là oracle cho test
(`crates/egret-perm/src/eval.rs:79`); ngữ pháp migration từ chối các khoá không thuộc dữ liệu (`UnknownLevel`,
`crates/egret-perm-compile/src/grammar.rs:166`, phát ra ở `:688`).

## Quyết định

| id | quyết định | bởi |
|---|---|---|
| E1 | Phạm vi là một **catalogue capability chọn lọc** (§2). Các capability gần như root không cấp được; chúng vẫn chỉ dành cho admin qua các policy được seed `egret-admin - <chain>`, và policy viết tay vẫn là lối thoát (F12) | user, 2026-10-18 |
| E2 | Catalogue Trino: impersonate một user có tên (đồng thời xem và kill query của họ, C1); đặt một system session property có tên; đặt một session property có tên của một catalog có tên; execute một procedure có tên; execute một catalog function có tên (C2) | quyết định mặc định (2026-10-18), tinh chỉnh từ danh sách của user |
| E3 | Catalogue StarRocks: impersonate một user có tên (`EXECUTE AS`, `AuthorizerStmtVisitor.java:1504-1516`); dùng một global function có tên (`sql/analyzer/AnalyzerUtils.java:301`); quản lý blacklist (`system` `blacklist`, `AuthorizerStmtVisitor.java:1296-1415`) | quyết định mặc định (2026-10-18); impersonation trên StarRocks thêm vào cho đối xứng với Trino |
| E4 | Không cấp được: Trino `trinouser` với wildcard, `role`, `sysinfo` write, `queryid` (C1); StarRocks `system` `node`/`plugin`/`file`/`grant`/`operate` (C4)/`repository`/mọi `create …`, `storage_volume`, `resource`, `resource_group` (C3). `GET /perm/engine-capabilities` liệt kê chúng kèm lý do | user (danh sách gần như root) + mặc định (C3, C4) |
| E5 | Chỉ allow (không deny). Principal: `user`, `group`, `role`; không bao giờ `workspace_members` (engine không gắn theo workspace) và không bao giờ group `public` (một capability engine cho tất cả mọi người là việc của các baseline được seed) | quyết định mặc định (2026-10-18) |
| E6 | Bảng mới `perm.engine_grants` và `perm.engine_grant_principals` (migration **0195**), không phải `perm.grants` (§3) | quyết định mặc định (2026-10-18) |
| E7 | Biên dịch thành một Ranger access policy cho mỗi engine grant, tên `perm-engine-{uuid}`, `managed_by = 'compiler'`, vào profile của engine (`engine_mgmt.engines.authz_service_id`); prefix này được thêm cạnh ba prefix hiện có ở mọi nơi chúng được liệt kê (§4.3) | quyết định mặc định (2026-10-18) |
| E8 | Giá trị là cụ thể: không `*`, `?`, `{`, `}`, không macro kiểu `{USER}`, không giá trị rỗng, kiểm tra ở route và bằng một `CHECK` | quyết định mặc định (2026-10-18) |
| E9 | Mọi response về engine grant đều mang `reach`: mọi engine bind vào profile mà grant biên dịch vào, để nhãn không bao giờ nói ít engine hơn số grant thực sự với tới (C5) | user, 2026-10-18 |
| E10 | Rebind của một engine provisioned cũng recompile profile trước đó; xoá engine recompile profile trước đó của nó khi còn engine khác bind vào (C11). Cả hai đều fire-and-forget sau commit, có log, như `restore_engine_authz_service` làm (`engines.rs:5201-5226`) | quyết định mặc định (2026-10-18) |
| E11 | Đích impersonation phải là một `users.username` hiện có (phân giải ra id của user và lưu theo id, E21), không bao giờ là `egret-lineage`, `egret_delegator` hay `root` (không phân biệt hoa thường; egret chạy dưới `root` trên StarRocks, security review C1, bị từ chối trên cả hai engine), không bao giờ là user đang giữ một cặp key/access không cấp được (gần-root) trên profile (security review I1: mọi dòng `NOT_GRANTABLE` trừ `queryid execute`, thứ ai cũng có, cộng `trinouser`/`user` với giá trị wildcard) qua bất kỳ access policy đang bật nào không phải engine grant đã biên dịch, kể cả policy của operator và infra, được nêu qua username, `{USER}`, một group họ thuộc về hoặc `public` (quét policy chứ không đánh giá: evaluator trả Undecidable cho mọi câu hỏi impersonation Trino trên profile delegating, C9, nên sẽ từ chối mọi đích); và không bao giờ là user mà từ đó một chuỗi bước impersonation cụ thể (security re-review I-a: qua mọi access policy đang bật của profile, kể cả engine grant đã biên dịch, và mô hình engine grant hiện tại, nên một grant chưa biên dịch cũng là một bước; `{USER}` là bước rỗng; giới hạn 256 user, đi xa hơn thì bị từ chối) với tới một user reserved hoặc một người giữ quyền gần-root, trên cả hai engine (ngữ nghĩa chuỗi của Trino chưa đo, nên mỗi bước được coi như danh tính hiện tại); tên group được so khớp không phân biệt hoa thường như Ranger (M-c); kiểm tra lại lúc biên dịch, đích nào không đạt thì bị bỏ qua và được báo cáo | quyết định mặc định (2026-10-18) |
| E12 | Các policy hạ tầng được seed của profile được liệt kê theo từng engine dưới dạng entry `managed` chỉ đọc, ánh xạ sang một capability khi tập khoá resource của chúng khớp (§5.2) | user, 2026-10-18 |
| E13 | `POST /perm/explain/capability` đánh giá một user và một capability trên artifact mà profile của engine phục vụ, bằng `egret-ranger-eval`; Undecidable được trả về đúng như vậy (C8, C9) | quyết định mặc định (2026-10-18), thay cho việc mở rộng `/perm/explain` |
| E14 | Route, quyền và hình dạng transaction như `write_grant`: đọc cần `policy:read`, ghi cần `policy:update`, một transaction giữ group lock, dòng engine (`FOR SHARE`), profile lock, dòng dữ liệu, và lần recompile (§5) | user, 2026-10-18 |
| E15 | Trang People lấy engine grant qua `GET /perm/engine-grants`, so khớp với `standsFor` ở phía client (C7) | quyết định mặc định (2026-10-18) |
| E16 | Engine grant được tính là tham chiếu ở mọi nơi grant được tính: từ chối xoá/đổi tên group, recompile khi đổi membership, sweep pre-mint, principal chưa phân giải, 409 khi cấp phát username (§4.5) | user, 2026-10-18 |
| E17 | Migrate (`/perm/policies/services/{id}/migrate`) không nhận diện policy mức engine viết tay; ngữ pháp vẫn từ chối các khoá không thuộc dữ liệu (`grammar.rs:166`) | quyết định mặc định (2026-10-18) |
| E18 | Audit: một hook có tên trong ba handler ghi, ghi log hình dạng `Mutation`; được nối với `egret_audit` bởi gói nào merge sau trong #3 và F11 (C12) | quyết định mặc định (2026-10-18) |
| E19 | Migration 0195 đổi tên mọi policy có sẵn có tên bắt đầu bằng `perm-engine-` mà không thuộc compiler, để sweep không bao giờ dọn mất một policy viết tay (§3.3) | quyết định mặc định (2026-10-18) |
| E20 | Ngoài phạm vi: tab "Engine permissions" (#4), deny, giới hạn theo workspace, dạy evaluator hiểu `{USER}`, RisingWave | user, 2026-10-18 |
| E21 | **Sửa đổi 2026-10-18 (controller, an toàn chặt):** ĐÍCH impersonation được lưu theo danh tính, `perm.engine_grants.target_user_id uuid REFERENCES users(id) ON DELETE CASCADE`, không bao giờ theo tên; compiler render `users.username` HIỆN TẠI của user từ id và bỏ qua grant khi user không có username. Vì thế một username được giải phóng rồi cấp lại cho người khác không bao giờ thừa hưởng một grant impersonation, và xoá user thì xoá luôn grant. Ghi nhận, không sửa (security re-review M-b): nhánh group của việc quét đích (I1, I-a) hiện không có tác dụng, vì không engine nào egret dựng phân giải group (D7) và một item group không khớp ai ở đó; nó chỉ từ chối thừa, và trở nên chính xác khi có engine phân giải group. Ghi nhận, không sửa: `egret_delegator` và `root` không phải username reserved (C6, security review C1; reserve chúng làm thay đổi việc tạo user và là việc tiếp theo, nên một người vẫn có thể được cấp một trong hai tên, và chỉ việc từ chối đích impersonation ngăn nó được dùng); principal được cấp quyền vẫn dùng cơ chế ref theo tên, nên username được giải phóng của một user bị xoá cứng vẫn có thể được cấp lại cho người khác, người này sẽ thừa hưởng các ref PHÍA ĐƯỢC CẤP (`crates/egret-perm-compile/src/principal_refs.rs:82-96`); update engine external recompile profile mới rồi profile cũ trong một transaction, mỗi cái qua `lock_services_in_tx` riêng của nó, nên hai profile lock không được lấy theo cùng một thứ tự tăng dần (`external.rs:406`, `:430-439`) | quyết định mặc định (2026-10-18), phần đích-theo-id do controller sửa đổi |

## 1. Những gì đang có

- **Mô hình và biên dịch.** `perm.grants` gắn theo catalog: `catalog_id … NOT NULL`, `actions <@ ARRAY['read','write']`
  (`migrations/0154_perm_model.sql:13-24`). `compile_desired` đọc mọi grant và rule, mở rộng các principal
  `workspace_members`, `role` và `group` khác `public` thành username (`trigger.rs:439-445`, `members.rs:668`), biên dịch
  các grant có catalog mà profile với tới (`trigger.rs:450-510`), rule, clearance, rồi từ chối các overlap được tạo ra
  (`trigger.rs:637-644`). `apply_diff_in_tx` khoá các service được sweep, đọc các policy thuộc compiler hiện có theo
  prefix, thực thi trần, đóng dấu `origin = 'generated'`, insert với `managed_by = 'compiler'`, update, dọn bỏ
  và tăng `policy_version` (`trigger.rs:975-1232`).
- **Đường ghi.** `write_grant` (`routes.rs:1047-1263`): giới hạn phạm vi, include không rỗng, đọc tập bị ảnh hưởng cũ,
  lấy group lock (`lock_named_groups`, `routes.rs:1018-1031`) TRƯỚC khi phân giải ref (`resolve_principal_refs`,
  `principal_refs.rs:167`), kiểm tra hình dạng, khoá profile theo thứ tự tăng dần (`lock_services_in_tx`, `trigger.rs:1833`), ghi,
  `recompile_services_in_tx` (`trigger.rs:1913`). Các handler create/patch/delete bọc nó trong một transaction
  (`routes.rs:1531-1640`).
- **Engine và profile.** `engine_mgmt.engines` có `id uuid`, `name`, `type IN ('trino','starrocks','risingwave')`
  (`migrations/0056_engines.sql:4-13`), `authz_service_id` nullable (`migrations/0058_engine_authz_service.sql:5-7`),
  `origin IN ('provisioned','external')` (`migrations/0184_engine_external.sql:20-24`). Một trigger tăng
  `policy_version` của các profile được bind mỗi khi bind thay đổi (`migrations/0187_view_lineage.sql:65-89`). Compiler đã
  join `engine_mgmt.engines` (`crates/egret-policy/src/store.rs:1336-1340`, `migrate.rs:448`).
- **Policy được seed.** Với mỗi engine Trino có delegation: `egret-user-impersonate-self` (`trinouser={USER}`, impersonate,
  `{USER}`), `egret-user-execute-query` (`queryid=*`, execute, `{USER}`), `egret-query-timeout`
  (`systemproperty=query_max_run_time`, alter, `{USER}`) (`delegator_policy.rs:78-125`), và trên các engine chế độ
  impersonate có `egret-delegator-impersonate` (`trinouser=*` cho `egret_delegator`, `:53-61`). Với mỗi profile:
  `egret-admin - <chain>` toàn quyền cho platform admin trên mọi cây resource (`default_policies.rs:53-107`,
  `:167-195`), và trên các profile Trino được seed qua `seed-defaults` có bộ `all - <chain>` cho group `admin`
  (`:116-160`). Tất cả đều `managed_by = 'infra'` (`infra_names.rs:93-100`).
- **Explain.** `explain_users` đánh giá artifact được phục vụ của từng profile (`served_artifact_via`, `routes.rs:2903`) với
  `EngineConfig { engine, plugin_version, ..without_super_users() }` và không có group hay role (`routes.rs:2096-2300`).
- **Chốt chặn group (#692).** Khi xoá hoặc đổi tên group, hệ thống hỏi `group_references_in_tx`, được trả lời bởi
  `group_references_via` trên principal của grant và các exemption của rule (`trigger.rs:1788-1810`), và từ chối kèm
  danh sách (`crates/egret-auth/src/groups.rs:312-355`). Các lần ghi membership recompile
  `affected_services_for_group_membership` / `_role_membership` (`trigger.rs:1722`, `:1574`).

## 2. Catalogue

`crates/egret-perm-compile/src/engine_capabilities.rs`, một bảng tĩnh. Mỗi entry gồm: `id`, loại engine, nhãn,
mô tả (người được cấp làm được gì, trong một hai câu), các khoá resource Ranger theo thứ tự cây, giá trị mà mỗi khoá
nhận (`username`, `identifier`, `catalog`, `function_signature`, hoặc `fixed "*"`), và một access type duy nhất.

| id | engine | resource → access | giá trị | đã kiểm chứng |
|---|---|---|---|---|
| `trino.impersonate_user` | trino | `trinouser` → `impersonate` | 1 username | `RangerSystemAccessControl.java:160-165` (set user), `:185-216` (xem, liệt kê, kill query của user) |
| `trino.set_system_session_property` | trino | `systemproperty` → `alter` | 1 identifier | `:235-240` |
| `trino.set_catalog_session_property` | trino | `catalog`, `sessionproperty` → `alter` | catalog, identifier | `:265-270` |
| `trino.execute_procedure` | trino | `catalog`, `schema`, `procedure` → `execute` | catalog, identifier, identifier | `:705-710` |
| `trino.execute_function` | trino | `catalog`, `schema`, `schemafunction` → `execute` | catalog, identifier, identifier | `:753-756`; function built-in và inline không bao giờ bị kiểm tra (`core/trino-main/.../metadata/FunctionResolver.java:365-370`) |
| `starrocks.impersonate_user` | starrocks | `user` → `impersonate` | 1 username | `AuthorizerStmtVisitor.java:1504-1516`, `authorization/ranger/starrocks/RangerStarRocksAccessController.java:55-62` |
| `starrocks.use_global_function` | starrocks | `global_function` → `usage` | 1 function signature | `AnalyzerUtils.java:301`; giá trị là `Function.getSignature()`, ví dụ `my_udf(INT,VARCHAR)` (`catalog/Function.java:955-968`) |
| `starrocks.manage_blacklist` | starrocks | `system` → `blacklist` | cố định `*` (`validationRegEx` của def là `\*`; plugin luôn hỏi `*`, `RangerAccessResourceBuilder.java:30-33`) | `AuthorizerStmtVisitor.java:1296-1415` |

Nhãn nêu rõ hệ quả: impersonation là "act as <user> on this engine: run queries with their permissions,
and see and kill their queries"; một procedure "runs with what the procedure itself checks; some connector procedures
change data without a further check"; execute function và procedure còn cần quyền truy cập catalog, thứ mà một data grant
cấp (`core/trino-main/.../security/AccessControlManager.java:1281-1283`); blacklist "affects every user of the
engine: it can block queries and take nodes out of scheduling".

**Không cung cấp, kèm lý do do API trả về (E4):** `trinouser` `*` hoặc mọi pattern (trở thành bất kỳ ai, kể cả
admin), `role` (quản lý role của Trino), `sysinfo` `write_sysinfo`, `queryid` (vốn đã là của mọi người, C1), StarRocks
`system` `node`, `plugin`, `file`, `grant`, `operate` (C4), `repository`, `create …`, `storage_volume`, `resource`,
`resource_group` (không có quyền usage, C3). Cũng không cung cấp (không ai yêu cầu, nhu cầu thấp): Trino `sysinfo`
`read_sysinfo`, `schemafunction` create/drop/show. Mỗi cái đều có thể thêm sau dưới dạng một dòng catalogue cùng các dòng test của nó.

**Quy tắc giá trị (E8, E11).** Mọi giá trị đều không rỗng, tối đa 255 ký tự, không có khoảng trắng đầu hay cuối, và
không chứa `*`, `?`, `{`, `}` (wildcard và ký hiệu macro của Ranger). Giá trị `username` phải phân giải được qua
`users.username` (citext, viết lại theo cách viết đã lưu, như `lookup_user` làm, `principal_refs.rs:321`), không được là
`egret-lineage` hay `egret_delegator`, và không được có tên trong `users` của một policy `egret-admin - …` đang bật trên
profile. Giá trị `catalog` phải là một catalog mà profile với tới (`catalogs_for_service_via`,
`crates/egret-policy/src/store.rs:1331`); giá trị `identifier` khớp `^[A-Za-z0-9_][A-Za-z0-9_.$-]*$`;
`function_signature` khớp `^[A-Za-z0-9_]+\([A-Z0-9_, .]*\)$`. Engine so khớp không phân biệt hoa thường cho các
khoá này (`ignoreCase: true` ở cả hai def), nên route lưu giá trị ở dạng chữ thường, trừ function signature, vì
StarRocks in kiểu đối số bằng chữ hoa.

## 3. Schema (migration 0195)

`migrations/0195_perm_engine_grants.sql`. Số này được giữ cho gói này (#3 dùng 0196 trở lên); kiểm tra lại
`origin/main` trước khi merge, như message của cổng tên file nói (`crates/egret-metastore/src/lib.rs:214-221`).

### 3.1 Bảng

```sql
CREATE TABLE perm.engine_grants (
    id          uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    engine_id   uuid NOT NULL REFERENCES engine_mgmt.engines(id) ON DELETE CASCADE,
    capability  text NOT NULL CHECK (capability IN (
                    'trino.impersonate_user', 'trino.set_system_session_property',
                    'trino.set_catalog_session_property', 'trino.execute_procedure',
                    'trino.execute_function', 'starrocks.impersonate_user',
                    'starrocks.use_global_function', 'starrocks.manage_blacklist')),
    resource_values text[] NOT NULL,
    target_user_id uuid REFERENCES users(id) ON DELETE CASCADE,
    created_at  timestamptz NOT NULL DEFAULT now(),
    updated_at  timestamptz NOT NULL DEFAULT now(),
    CHECK (array_position(resource_values, NULL) IS NULL),
    CHECK (array_position(resource_values, '') IS NULL),
    CHECK (array_to_string(resource_values, chr(31))
           !~ ('(^|' || chr(31) || ')\s|\s($|' || chr(31) || ')')),
    CHECK (array_to_string(resource_values, chr(31)) !~ '[*?{}]'),
    CHECK (cardinality(resource_values) = CASE capability
             WHEN 'trino.set_catalog_session_property' THEN 2
             WHEN 'trino.execute_procedure' THEN 3
             WHEN 'trino.execute_function' THEN 3
             WHEN 'trino.impersonate_user' THEN 0
             WHEN 'starrocks.impersonate_user' THEN 0
             WHEN 'starrocks.manage_blacklist' THEN 0
             ELSE 1 END),
    CHECK ((capability IN ('trino.impersonate_user', 'starrocks.impersonate_user'))
           = (target_user_id IS NOT NULL)),
    UNIQUE NULLS NOT DISTINCT (engine_id, capability, resource_values, target_user_id)
);
CREATE INDEX engine_grants_engine_idx ON perm.engine_grants (engine_id);

CREATE TABLE perm.engine_grant_principals (
    engine_grant_id uuid NOT NULL REFERENCES perm.engine_grants(id) ON DELETE CASCADE,
    kind  perm.principal_kind NOT NULL CHECK (kind IN ('user', 'group', 'role')),
    ref   text NOT NULL CHECK (ref <> ''),
    CHECK (NOT (kind = 'group' AND ref = 'public')),
    PRIMARY KEY (engine_grant_id, kind, ref)
);
CREATE INDEX engine_grant_principals_ref_idx ON perm.engine_grant_principals (kind, ref);
```

Dùng `resource_values` chứ không phải `values`, vì `VALUES` là từ khoá reserved. Capability impersonation lưu đích ở
`target_user_id` và không có `resource_values` (E21, đã sửa đổi): API vẫn nhận và trả username, route phân giải nó ra id
và mọi lần đọc render lại từ id. Không có cột `mode`: chỉ allow, chỉ include (E5).
`manage_blacklist` không lưu giá trị nào; compiler tự cung cấp `*`. Danh sách capability trong `CHECK` và catalogue
Rust là một sự thật ở hai nơi, nên một test DB insert mọi id trong catalogue và một id không tồn tại (§7).

**Vì sao không dùng `perm.grants`.** `catalog_id` của nó là `NOT NULL` và mọi nơi đọc đều giả định như vậy: tập service bị
ảnh hưởng được suy ra từ nó (`trigger.rs:1403-1416`), `covers`/targeting và các tab của Data lọc theo nó, trang People nhóm
grant theo scope, bộ từ vựng action là một `CHECK` (`0154_perm_model.sql:23`), và ngữ pháp migration biên dịch
về nó. Nới lỏng bất kỳ điều nào trong số này đều đụng tới mọi nơi đọc đó vì những dòng mà không nơi nào trong số đó dùng được.

### 3.2 Vòng đời của dòng engine làm gì

`ON DELETE CASCADE` từ engine: xoá một engine sẽ xoá các engine grant của nó. Các policy đã biên dịch trong profile
trước đó của nó được dọn bởi lần recompile kế tiếp của profile đó (E10).

### 3.3 Giữ chỗ prefix (E19)

Trước migration này, `perm-engine-` là một tên bình thường mà operator có thể dùng; sau nó, sweep dọn mọi
policy mang prefix này mà không engine grant nào sinh ra (`trigger.rs:1016-1031`). Migration đổi tên mọi dòng
`policy.authz_policies` có tên bắt đầu bằng `perm-engine-` và có `managed_by` khác `compiler`, thành
`renamed by 0195: <name>`, đặt `policy_jsonb->'name'` thành cùng chuỗi đó, tăng `version` của dòng và `policy_version`
của service của nó, và `RAISE NOTICE` số lượng. (Trước gói này không dòng nào thuộc compiler có thể mang prefix.) Một test DB
chạy câu lệnh trên một fixture (§7).

## 4. Biên dịch

### 4.1 Mô hình

`egret-perm` có thêm `engine_grants.rs`: `EngineGrant { id, engine_id, capability: String, values: Vec<String>,
principals: PrincipalSet /* exclude always empty */ }` và các hàm generic theo connection `list_engine_grants_via`,
`list_engine_grants_for_profile_via(conn, service_id)` (join `engine_mgmt.engines` trên `authz_service_id`),
`get_engine_grant_via`, `create_engine_grant_in_tx`, `update_engine_grant_in_tx`, `delete_engine_grant_in_tx` —
cùng cách chia như `list_grants_via`/`create_grant_in_tx` (`crates/egret-perm/src/store.rs`). `egret-perm` không phụ thuộc
`egret-policy` (`Cargo.toml:13-17` của nó nói lý do); catalogue nằm trong `egret-perm-compile`.

### 4.2 compile_engine_grant

`egret_perm_compile::engine_grants::compile_engine_grant(g, profile_name, engine) -> Result<AccessPolicyInput,
CompileError>`, thuần: resource lấy từ entry catalogue và `g.values` (`isExcludes: false, isRecursive: false`),
một allow item với các username đã mở rộng và access type duy nhất, `delegateAdmin: false`, render qua
`access_to_ranger_json` và `stamp_service_type` như grant (`trigger.rs:477-481`). Nó từ chối loại engine
không khớp với capability, giá trị vi phạm quy tắc của §2, và ref `User` không qua được `validate_user_ref`.

`compile_desired` (`trigger.rs:373`) thêm, sau phần grant: đọc `list_engine_grants_for_profile_via(conn,
service_id)`, mở rộng tập principal của chúng trong cùng lần gọi `expand_enumerated_principals` với grant và rule
(hàm này có thêm đối số `engine_grants: &mut [EngineGrant]` và `referenced_principals` quét qua chúng), rồi với từng grant:

- include rỗng sau khi mở rộng → `SkippedItem { source_kind: EngineGrant, … }`, như một grant (`trigger.rs:454-475`);
- một đích impersonation không còn phân giải được, là reserved, hoặc nay có tên trong một policy `egret-admin - …` của
  profile → bỏ qua kèm lý do đó (E11). Không phải lỗi: một user bị xoá ở nơi khác không được làm hỏng mọi lần recompile
  của profile;
- ngược lại thì insert vào `desired` với khoá `(service_id, "perm-engine-{id}")`, `source_kind: EngineGrant`, không có
  dòng provenance (C10, mở rộng nhánh return sớm của `record_compiled`).

Policy được tính vào `PROFILE_EXPANSION_CEILING` như mọi policy desired khác (`trigger.rs:1052-1066`); một
engine grant là một policy, nên nó chỉ có thể đẩy profile vượt trần, không bao giờ nhân lên.

### 4.3 Prefix, ở mọi nơi nó được liệt kê (E7)

`egret_policy::COMPILER_ENGINE_GRANT_PREFIX = "perm-engine-"` đặt cạnh các prefix khác (`store.rs:87-112`), và:

1. `is_compiler_owned_policy_name` (`store.rs:119-123`);
2. message của `refuse_reserved_name` (`store.rs:145-156`);
3. phép đếm của `refuse_if_compiler_owned_policies_remain` (`store.rs:644-671`);
4. query trạng thái hiện có của `apply_diff_in_tx` (`trigger.rs:1016-1031`);
5. `attribute_policy_name` → `PolicyAttribution::EngineGrant(Uuid)` (`crates/egret-perm-compile/src/attribution.rs:91-106`),
   và `ExplainAttribution::EngineGrant { engine_grant_id }` (`routes.rs:503-554`).

`grep -rn "COMPILER_CLEARANCE_PREFIX" crates` là checklist: mọi chỗ production liệt kê prefix clearance
đều liệt kê cả prefix này. `managed_by_agreement.rs` giữ nguyên (nó ghim backfill của 0180, có trước prefix này); việc
đổi tên của 0195 có test riêng.

### 4.4 Một lần ghi recompile những profile nào

Một engine grant với tới đúng một profile: `authz_service_id` của engine của nó. Create và delete recompile profile đó; patch
không thể chuyển grant giữa các engine (body không có `engine_id`), nên nó recompile cùng profile đó. Engine không có
profile, có loại `risingwave`, hoặc có `status = 'deleting'` sẽ từ chối lần ghi (409, kèm lý do).

### 4.5 Tham chiếu (E16)

| nơi đọc | thay đổi |
|---|---|
| `group_references_via` (`trigger.rs:1788-1810`) | nhánh `UNION ALL` thứ ba trên `perm.engine_grant_principals`, `source = 'engine_grant'`, `effect = 'allow'`, `position = 'include'` |
| doc của `GroupReference.source` và chữ từ chối (`crates/egret-auth/src/groups.rs:169-185`, `:336-344`) | `"engine_grant"` → `engine grant <id> (allow, include)` |
| `affected_services_for_group_membership` (`trigger.rs:1722`) và `_role_membership` (`:1574`) | thêm profile của các engine có grant nêu tên group / role |
| `premint_model_usernames` (`members.rs:357-367`) | `UNION SELECT kind::text, ref FROM perm.engine_grant_principals` |
| `unresolved_principals_via` (`principal_refs.rs:447-459`) | một nhánh `engine_grant`; `failsOpen` luôn là false (allow include) |
| `stored_user_refs_naming_via` (`principal_refs.rs:637-650`) | principal `user` của engine grant (`source = 'engine_grant'`). Không có nhánh cho đích impersonation: đích được lưu theo user id (E21, đã sửa đổi), nên không có tên lưu trữ nào có thể cũ đi |
| `UserRepo::hard_delete` (`crates/egret-auth/src/users.rs:295`), qua `GroupMembershipRecompiler::lock_affected_by_user_delete_in_tx` | khoá và recompile thêm profile của các engine grant có ĐÍCH là user đó (E21, đã sửa đổi): grant bị cascade theo user, và cùng transaction đó dọn các policy đã biên dịch của chúng, vốn nêu username mà lần xoá giải phóng |
| `recompile_role_referencing_profiles` (`crates/egret-perm-compile/src/role_membership.rs:251`) | principal `role` của engine grant được đưa vào lượt quét role lúc khởi động |
| tạo/sửa/bật/xoá trong trình sửa policy (`crates/egret-policy/src/admin/mod.rs`, `write_policy_checked`, `fold_profile_of_stored`) | security re-review M-a: trên profile có engine mang grant impersonation, mọi lần ghi của trình sửa khoá các dòng unit của profile và recompile nó trong cùng transaction, nên một đích mà lần ghi biến thành gần-root (hoặc cho thêm một bước) mất policy `perm-engine-` ngay. Seed infra: một platform admin (người được `seed_admin_all_access` seed làm engine admin ở mỗi lần apply) bị từ chối thẳng làm đích, hay ở bất kỳ đâu trên chuỗi, và việc gán hoặc thu hồi role `platform_admin` recompile mọi profile mang grant impersonation trong chính transaction của nó (`affected_services_for_role_membership`), nên một đích trở thành platform admin mất grant ngay trong commit đó; bản thân các đường seed (`seed_admin_all_access`, delegator, baseline per-user) recompile profile như vậy ngay sau khi seed, best-effort và có log, vì các upsert của chúng không nằm trong một transaction |

Các đường recompile theo workspace-member và tag không cần đổi: engine grant không nêu tên workspace hay tag nào.

## 5. API

Tất cả dưới `/api/control-plane`, trong `crates/egret-perm-compile/src/routes/engine_grants.rs`, được mount từ
`perm_router` (`routes.rs:120-166`) và đăng ký trong `PermApi` (`routes.rs:176-315`). Đọc chặn bởi `policy:read`,
ghi bởi `policy:update`, qua `egret_core::authz::require` như `list_grants` (`routes.rs:1502`). DTO dùng camelCase.

### 5.1 Catalogue

`GET /perm/engine-capabilities` → `{ engines: [{ engineType, capabilities: [Capability], notGrantable: [{ what,
why }] }] }`, `Capability = { id, label, description, resources: [{ key, valueKind }], access }`.

### 5.2 Theo engine

`GET /perm/engines/{engine_id}/grants` →

```text
{ engine: { id, name, type, origin, status },
  profile: { serviceId, name } | null,
  reach: { engines: [{ id, name, origin }], sharedProfile: bool },
  grants: [EngineGrantView],
  managed: [ManagedEntry] }
EngineGrantView = { id, engineId, engineName, capability, values, targetUserId, principals: [PermPrincipal], policyName, reach }
ManagedEntry    = { policyId, policyName, kind: user_baseline | delegator | admin_all_access | default_all_access,
                    capability: string | null, resources: {key: [value]}, accesses: [string],
                    users: [string], groups: [string], enabled }
```

`reach` liệt kê mọi engine có `authz_service_id = profile`, sắp theo tên (E9). `grants` liệt kê mọi engine grant với tới engine: trên một profile dùng chung kiểu cũ, gồm cả các grant được viết trên engine khác của profile, mỗi grant được đánh dấu bằng `engineId`/`engineName` (security review M3). `managed` liệt kê mọi policy
`managed_by = 'infra'` của profile; `capability` là id catalogue có tập khoá resource bằng tập khoá của
policy, ngược lại là `null` (các cây dữ liệu và `queryid`); `kind` lấy từ tên (các hằng trong `infra_names.rs`). Engine
không có profile trả về `profile: null`, `grants`/`managed` rỗng, và 200. Engine không tồn tại → 404.

`GET /perm/engine-grants` → mọi engine grant (`EngineGrantView`), cho trang People (E15).

### 5.3 Ghi

`POST /perm/engines/{engine_id}/grants` với `NewEngineGrant { capability, values, principals: [PermPrincipal] }`
→ 201 `{ id, compile: CompileResponse, reach }`. `PATCH /perm/engine-grants/{id}` với `{ capability, values,
principals }` (toàn bộ body; engine là cố định) → 200 `{ compile, reach }`. `DELETE /perm/engine-grants/{id}` →
200 `{ compile, reach }`.

`write_engine_grant(tx, engine_id, id, body, p)`, một cài đặt duy nhất cho create và patch, theo thứ tự này:

1. đọc engine; 404, hoặc 409 khi nó không có profile, là `risingwave`, đang `deleting`, hoặc loại engine của
   capability khác (400 cho trường hợp cuối);
2. kiểm tra giá trị theo catalogue (400 kèm quy tắc bị vi phạm); principal không rỗng, kind là `user`/`group`/
   `role`, không có `public` (400);
3. `lock_named_groups` trên principal của body và của dòng đã lưu (group lock trước, `routes.rs:1018-1031`);
4. `resolve_principal_refs` (`principal_refs.rs:167`), với keep-set R6 cho patch đúng như `write_grant`;
5. `SELECT authz_service_id FROM engine_mgmt.engines WHERE id = $1 FOR SHARE`, đọc lại dưới khoá: một lần rebind
   (`set_engine_authz_service_in_tx`, hàm UPDATE dòng đó) giờ phải chờ transaction này, nên grant không thể được
   ghi vào một profile mà engine vừa rời. `FOR KEY SHARE` sẽ không xung đột với UPDATE đó;
6. đích impersonation: phân giải (`users.username`), từ chối đích reserved và đích admin (409 nêu lý do);
   giá trị `catalog`: từ chối catalog mà profile không với tới (400 liệt kê các catalog nó với tới);
7. `lock_services_in_tx(tx, &[profile])` (mở rộng kèm tag service, `trigger.rs:1833`);
8. insert hoặc update (vi phạm unique → 409 "this engine already has that grant");
9. `recompile_services_in_tx(tx, &[profile])`.

Thứ tự khoá là thứ tự J-F3 cộng thêm dòng engine: group lock, dòng engine, profile lock. Đường rebind lấy
dòng engine rồi tới profile (`engines.rs:5399`, rồi `:5410`), và không đường nào lấy profile lock trước dòng engine, nên không
thêm chu trình nào.

### 5.4 Explain (E13)

`POST /perm/explain/capability` với `{ subject: { user }, engineId, capability, values }` →
`{ engine, profile, reach, outcome: per_profile | no_profile, decision: WireDecision | null, attribution:
ExplainAttribution | null, policyName: string | null, tagResolution: "not_applicable" | "declined" }`.

Cùng quy tắc subject như `ExplainSubject` (`routes.rs:455-457`): một username, không gửi group hay role. Artifact là
`served_artifact_via` của profile của engine; `EngineConfig` dựng như `explain_users` dựng
(`routes.rs:2207-2211`); request là resource của entry catalogue với `values`, access type của entry,
`owner: None`.

Tag: một capability có resource không có level `catalog` (`trinouser`, `systemproperty`, StarRocks `user`,
`global_function`, `system`) được hỏi với `resolved_tags: Some(∅)`, vì service tag của egret chỉ bao giờ mang các phần tử
resource `catalog`/`schema`/`table`/`column` (`crates/egret-policy-api/src/dto.rs:142-204`), nên không tag nào có thể
gắn vào resource như vậy. Điều này được lập luận từ phía ghi của egret, không được đo: differential bỏ qua enricher
(`crates/egret-ranger-eval/tests/differential.rs:56-73`). Một capability có level `catalog` được hỏi với `None`
mỗi khi khối tag của bản tải về chứa một policy, và `TagsUnresolved` của evaluator được trả về dưới dạng Undecidable,
`tagResolution: "declined"`.

Mọi câu hỏi `trino.impersonate_user` của Trino trên profile có delegation đều Undecidable (C9). Response mang
nguyên nhân từ evaluator; nhãn cần hiển thị là "egret cannot decide impersonation on this engine: its baseline lets every
user become themself through `{USER}`, which egret's evaluator does not expand".

### 5.5 Mã trạng thái

400: capability không tồn tại, sai loại engine, giá trị vi phạm quy tắc, principal rỗng hoặc không hợp lệ, ref không
phân giải được. 403: thiếu quyền. 404: không có engine / không có grant. 409: engine không có profile, `deleting`, `risingwave`;
đích impersonation reserved hoặc admin; grant trùng; từ chối do overlap hoặc trần từ lần recompile (các ánh xạ
hiện có). 500: lỗi nội bộ.

## 6. Vòng đời engine (E10)

- `update_engine` (`engines.rs:5531-5547`): sau khi `apply_authz_service_update` commit một lần rebind, recompile
  profile trước đó bằng `recompile_profile` (fire-and-forget, ghi log kèm URL sửa chữa, như
  `restore_engine_authz_service` ở `engines.rs:5212-5226`). Không nằm trong transaction của việc bind: port recompile
  một profile mỗi lần gọi, mỗi lần với `lock_services_in_tx` riêng, nên hai lần gọi sẽ không lấy cả hai khoá theo cùng
  một thứ tự tăng dần (đúng hình dạng mà E21 ghi nhận cho `external.rs`). Từ I2, profile trước đó không còn engine nào
  khác, nên khoảng hở sau commit không phục vụ ai ngoài một profile dùng chung dạng cũ. Engine external không cần gì:
  update của chúng đã recompile cả hai profile.
- `delete_engine` (`engines.rs:5953-5965`) và `delete_external_engine` (`engines.rs:5979-6011`): sau khi dòng
  bị xoá, nếu còn engine khác bind vào profile trước đó (`count_engines_bound_to_authz_service`,
  `crates/egret-engine-mgmt/src/store.rs:378-389`), recompile nó. Khi không còn engine nào thì không ai tải profile về,
  và lần bind kế tiếp sẽ recompile nó (`engines.rs:5367-5440`), việc này dọn các policy cũ.

## 7. Audit hook (E18)

Mỗi handler ghi gọi `audit_engine_grant_write(&p, action, id, &detail)` sau commit, trong đó `action` là
`perm.engine_grant.create|update|delete`, `resource_type` là `engine_grant`, và `detail` là `{ engineId, capability,
values, principals, services }`. Hôm nay hàm này phát `tracing::info!(target: "egret::audit", …)` với các
trường đó, nên nó không phải dead code và hình dạng được ghim bằng một unit test. #3 thêm một audit sink vào `PermState`; gói
nào merge sau trong #3 và gói này sẽ thay thân hàm bằng `st.audit.emit(egret_audit::mutation_for(p, None, action,
"engine_grant", Some(id), detail))`, đúng hình dạng mà `crates/egret-audit/src/lib.rs:34-51` dựng.

## 8. Kiểm thử

- **Thuần (catalogue).** Với mọi entry: mỗi khoá resource tồn tại trong service-def của engine của nó, theo thứ tự cây;
  access type nằm trong `accessTypes`; nó nằm trong `accessTypeRestrictions` của lá, trừ `trino.execute_function`,
  được khẳng định là ngoại lệ duy nhất (C2). Không entry nào dùng `role`, `sysinfo`, `queryid`, `storage_volume`,
  `resource`, `resource_group`, hoặc access `system` nào khác `blacklist`; không value kind nào của entry chấp nhận `*`.
  Mọi dòng `notGrantable` đều nêu tên một khoá hoặc access mà các def khai báo. Các id capability bằng danh sách `CHECK`
  của migration (đọc bằng `include_str!`).
- **Thuần (biên dịch).** Body cho từng capability; các từ chối theo quy tắc giá trị (`*`, `?`, `{USER}`, rỗng, khoảng trắng);
  các từ chối của `validate_user_ref`; không khớp loại engine.
- **DB (`egret_test_db::url()`, giới hạn trong các service id của chính fixture).** Migration 0195: bảng, các `CHECK`,
  việc đổi tên một dòng viết tay `perm-engine-x` được seed sẵn (tên, tên trong body, version được tăng) và một
  tên thuộc compiler không bị đụng tới. Biên dịch: một grant biên dịch đúng một lần vào profile của engine của nó với
  `managed_by = 'compiler'` và `origin = 'generated'`; delete dọn nó; một group grant mở rộng thành thành viên và recompile
  khi membership đổi; role grant cũng vậy; group rỗng thì bỏ qua; đích impersonation đã bị xoá thì bỏ qua kèm
  lý do; trần có tính engine grant. Profile dùng chung (dạng cũ, tạo bằng SQL): `reach` liệt kê cả hai engine và policy
  nằm trong một profile duy nhất. Rebind: profile cũ mất policy, profile mới có nó. Xoá engine: cascade, và
  profile trước đó dùng chung được recompile. Tham chiếu: xoá và đổi tên group bị từ chối khi có engine grant nêu tên
  group (đường của #692); `/perm/unresolved-principals` liệt kê ref cũ của một engine grant; cấp phát username
  trả 409 cho một giá trị impersonation cũ; sweep pre-mint mint thành viên của group được cấp.
- **Route với `CedarPdp` thật** (`crates/egret-perm-compile/tests/people.rs:131-172` là mẫu): mọi route
  403 khi thiếu quyền của nó, 200/201 khi có; từng mã 400/404/409 của §5.5; `FOR SHARE` tuần tự hoá một lần rebind đồng thời
  (mẫu `tests/support/pausing.rs`).
- **Explain.** Trino `set_system_session_property` được allow cho người được cấp và bị deny cho user khác;
  impersonation Undecidable trên profile có delegation kèm nguyên nhân; StarRocks `use_global_function` được allow;
  `execute_procedure` declined khi khối tag có policy.
- **Differential (`EGRET_TEST_RANGER_E2E=1`).** Một test mới trong `crates/egret-ranger-eval/tests/differential.rs`,
  theo mẫu `an_editor_saved_table_policy_grants_select_at_column_depth_on_the_real_jars` (`:8413`), hỏi các engine thật
  2.5.0, 2.8.0 và 2.9.0 cùng crate này những case giống nhau, với body dựng bằng `compile_engine_grant`: Trino
  `trinouser` (người được cấp làm đích, làm user khác, các biến thể hoa thường), `queryid` (`*` được seed, để ghim baseline của C1),
  `systemproperty`, `catalog`+`sessionproperty`, `catalog`+`schema`+`procedure`, `catalog`+`schema`+`schemafunction`
  với `execute` (câu hỏi về restriction của C2, trên engine thật); StarRocks `resource_group` (`alter` được allow bởi một
  policy tường minh, để ghim rằng khoá này được đánh giá), `global_function` `usage`, `system` `blacklist` và `operate` (đối
  chứng). Mỗi case đều có một đối chứng không phải người được cấp.

## 9. Ngoài phạm vi

Tab Engine permissions (#4); deny; giới hạn theo workspace; RisingWave; mở rộng `{USER}` trong evaluator; migrate
policy mức engine viết tay (E17); liệt kê policy mức engine viết tay theo từng engine (F12, #4); hai
lỗ hổng đã ghi nhận ở E21.
