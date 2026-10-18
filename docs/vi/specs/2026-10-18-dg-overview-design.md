# Thiết kế lại frontend Data Governance #3: Governance Overview — thiết kế

> Bản tham khảo tiếng Việt. Bản tiếng Anh `docs/en/specs/2026-10-18-dg-overview-design.md` là bản chuẩn; hai bản lệch nhau thì theo bản tiếng Anh.

Ngày: 2026-10-18. Trạng thái: bản nháp trên khung chương trình (quyết định F1–F12 của
`docs/en/specs/2026-10-16-dg-frontend-redesign-design.md`, dưới đây gọi là "spec chương trình"). Base: main `4a67321b`
(#2 People & groups đã merge là #690, gói group-name/role-mint là #692). Nhánh `feat/dg-overview`, worktree `egret-dgov`.

Phần 1 của spec chương trình cho sub-project này một dòng: **Overview — những gì cần chú ý — mới.** Tài liệu này đặc
tả nó. Mọi khẳng định về code hiện có đều kèm `file:line` tại `4a67321b`.

Phạm vi do user chốt ngày 2026-10-18: các tín hiệu egret đã tính sẵn, cộng độ lệch của engine (plugin không giữ cái
egret phục vụ), cộng nhật ký audit cho governance. Thay đổi gần đây được phục vụ bởi một route mới chỉ trả sự kiện
governance, gate `policy:read` HOẶC `tag:read`; audit log đầy đủ vẫn là `audit:read`
(`crates/egret-controlplane/src/audit.rs:80`).

## Các tiền đề trong brief mà code mâu thuẫn

Brief đã được đối chiếu với code. Những câu sau trong brief sai hoặc thiếu; thiết kế đi theo code:

1. **"Ghi sự kiện trong cùng transaction với thao tác ghi, nếu store hiện có cho phép."** Store hiện có không bao giờ
   cho phép. Mọi sự kiện mutation hôm nay đi qua `AuditSink::emit`, một kênh `mpsc` có giới hạn được một batch writer
   rút (`crates/egret-audit/src/lib.rs:53-71`, `:217-256`). `emit` được gọi **sau** khi thao tác ghi đã commit
   (`crates/egret-controlplane/src/access/mod.rs:466`, `crates/egret-bi/src/catalog_tags.rs:683`), bỏ sự kiện khi kênh
   đầy (`lib.rs:64-69`), và bỏ mọi sự kiện khi `audit.enabled = false` vì lúc đó không ai rút receiver
   (`crates/egret-bin/src/main.rs:1201-1210`). Ghi theo transaction cần một helper mới (§3.1).
2. **`egret-auth/src/pdp/audit.rs` không phải nơi ghi mutation.** Đó là decorator của PDP, ghi một dòng `decision`
   cho mỗi lần phân quyền (`crates/egret-auth/src/pdp/audit.rs:27-37`). Mutation dùng chung bảng `audit.audit_log`
   (`migrations/0076_audit_log.sql:5-17`) với `kind = 'mutation'`. Vì vậy decision chiếm phần lớn bảng, điều này ảnh
   hưởng tới truy vấn của feed (§3.4).
3. **"Dòng bị xoá sau 24 h" (`policy.ranger_plugin_downloads`).** Dòng im lặng chỉ bị dọn theo cơ hội: cứ 64 lần
   upsert *cho cùng service* thì một lần xoá các dòng cũ hơn 24 h của service đó
   (`crates/egret-policy/src/plugin_downloads.rs:96-108`, `:113-128`). Service mà mọi plugin đã ngừng poll thì không có
   upsert nào, nên dòng cũ ở lại. Do đó các luật độ lệch (§4.3) đọc `at`, không bao giờ đọc sự có mặt của dòng.
4. **`ranger_plugin_version_modelled` không được lưu.** Nó được tính cho mỗi request bằng Rust từ type, origin,
   version khai báo hoặc image đã pin của engine (`plugin_report`, `crates/egret-engine-mgmt/src/engines.rs:335-371`).
   Không đọc được bằng SQL; §4.4 chuyển hàm này sang `egret-perm` để Overview gọi được.
5. **PUT access-policy của tag không ở `tags.rs:243`.** Nó là `set_access_policy`
   (`crates/egret-controlplane/src/tags.rs:285`), và có một anh em, `DELETE` `clear_access_policy` (`:334`); cả hai đều
   được audit ở đây. Thao tác ghi thật là `ClassificationPolicyRepo::set_policy`/`clear_policy`, tự mở transaction
   (`crates/egret-bi/src/tags.rs:514-563`, `:572-586`).
6. **Trích dẫn nhỏ.** `/perm/unresolved-principals` và `/perm/overlaps` là route trong `routes.rs` (`:1687`, `:1656`);
   `principal_refs.rs:377` và `overlap.rs:432` là DTO, còn hàm store là `unresolved_principals_via`
   (`principal_refs.rs:447`) và `stored_overlaps_via` (`overlap.rs:478`). Comment giữ chỗ cho Overview nằm ở
   `frontend/src/components/layout/Sidebar.tsx:83-85`.
7. **Heading của sidebar không thể là "Overview".** Control Plane đã có một nhóm tên "Overview" (`Sidebar.tsx:119`),
   và trạng thái thu gọn nhóm là một khoá `localStorage` chung cho cả ba app, nên heading phải khác nhau
   (`Sidebar.tsx:79-80`, `:157-158`).
8. **Sự kiện role và tag catalog hiện có không dùng thẳng được như đang ghi.** Sự kiện role mang id của role, không
   mang tên (`access/mod.rs:466-477`). `tag:assign`/`tag:unassign` được phát cho cả cột catalog
   (`resource_type = 'catalog'`, `catalog_tags.rs:683-692`) lẫn object BI trong workspace
   (`resource_type = 'object'`, `crates/egret-bi/src/shares.rs:615-622`); chỉ loại đầu là governance.
9. **Lỗi compile phần lớn không để lại gì để lưu.** Gần như mọi lần compile chạy trên transaction của caller và một
   lỗi sẽ rollback thao tác ghi của caller (`recompile_services_in_tx`, `crates/egret-perm-compile/src/trigger.rs:1902-1945`).
   Profile khi đó vẫn giữ lần compile trước; không có gì hỏng *trên engine*. §3.3 chỉ ghi lỗi ở nơi không có thao
   tác ghi nào bị từ chối cùng với nó.

## Quyết định

| id | quyết định | bởi |
|---|---|---|
| O1 | Phạm vi: tín hiệu có sẵn + độ lệch engine + audit governance. Loại ra: audit DEFINER của view-exposure (nó gọi engine; `crates/egret-controlplane/src/view_exposure.rs`), chỉ đặt link; độ phủ classification | user, 2026-10-18 |
| O2 | `GET /api/control-plane/governance/changes` chỉ trả sự kiện governance, gate `policy:read` HOẶC `tag:read`; mỗi họ sự kiện chỉ trả cho người đọc giữ quyền của họ đó (§3.4). `/audit` giữ `audit:read` | user, 2026-10-18 (gate); quyết định mặc định (lọc theo họ) |
| O3 | Sự kiện governance được ghi **trong chính transaction của thao tác ghi** bởi `egret_audit::governance::record_change` mới, vào `audit.audit_log` với `kind = 'mutation'`. Thao tác bị từ chối, rollback, preview hay dry-run không để lại sự kiện. Chúng được ghi bất kể `audit.enabled`; cờ này vẫn điều khiển dòng decision và các sự kiện sink hiện có, và doc comment của nó nói rõ điều đó | user, 2026-10-18 |
| O4 | Sự kiện role và tag catalog hiện có vẫn đi qua sink (best effort, sau commit); feed đọc chúng và nói rõ chúng là best effort | quyết định mặc định (2026-10-18) |
| O5 | Không backfill. Việc ghi bắt đầu khi phiên bản này được triển khai; trạng thái rỗng và chú thích của feed nói vậy | user, 2026-10-18 |
| O6 | `perm.compile_runs` (migration 0196): lần compile gần nhất của mỗi profile. Thành công do phễu compile ghi trong transaction của lần compile; thất bại chỉ do các điểm vào không có thao tác ghi nào chờ (§3.3) | quyết định mặc định (2026-10-18) |
| O7 | `GET /api/control-plane/governance/attention`, gate `policy:read` HOẶC `tag:read`; mỗi phát hiện chỉ được tính cho người đọc giữ quyền của nguồn, nếu không thì nằm trong `notChecked` | user, 2026-10-18 |
| O8 | Mỗi nguồn chạy trong savepoint riêng của một transaction `REPEATABLE READ, READ ONLY` với `statement_timeout` 5 s; nguồn lỗi trở thành phát hiện `source_unavailable` | quyết định mặc định (2026-10-18) |
| O9 | Độ lệch: `plugin_silent` (critical, không poll trong 24 h), `plugin_behind` (critical, version chưa được xác nhận sau 90 s kể từ khi đặt), và thêm `plugin_quiet` (warning, không poll trong 5 phút) để engine đã dừng không vô hình suốt một ngày | quyết định mặc định (2026-10-18); ngưỡng do Task 1 xác nhận (`2026-10-18-dg-overview-live.md` §1.1), Task 1 cũng làm rõ luật: lần poll mới nhất của mỗi profile, `-1` là "chưa báo", kind không có lần tải gần đây thì không xét (§4.3) |
| O10 | Số migration 0196 (`perm.compile_runs`) và 0197 (partial index trên mutation của audit). F11 giữ 0195 | user, 2026-10-18 |
| O11 | Frontend: khu lazy `routes/governance/overview/`, là index của Governance; catch-all `*` chuyển hướng về `/governance` (Overview); nhóm sidebar tên **"Governance overview"**, mục "Overview", đứng đầu | quyết định mặc định (2026-10-18) |
| O12 | Không biểu đồ. Dải số đếm, Needs attention (theo mức độ), Recent changes (phân trang, 25 mỗi trang) | user, 2026-10-18 |
| O13 | Ngoài #3: audit cho CRUD tag, tạo/xoá profile và seed, thành viên workspace; actor hệ thống cho các sweep và sync workspace-assignment; quy plugin về từng engine; cache | quyết định mặc định (2026-10-18) |

## 1. Những gì đã có

- **Audit.** `audit.audit_log(id, ts, kind, actor_user_id, impersonator, workspace_id, action, resource_type,
  resource_id, decision, reasons, detail)` (`migrations/0076_audit_log.sql:5-17`, `0095_impersonations.sql:19`), có
  index trên `(ts, id)`, `(actor, ts)`, `(action, ts)` (`0076:19-21`). `AuditStore::query` lọc mỗi lần một `action`
  (`lib.rs:166-205`). Dòng cũ hơn `audit.retention_days` (mặc định 90) bị dọn mỗi giờ, và retention bằng 0 hoặc nhỏ
  hơn nghĩa là không dọn (`crates/egret-config/src/lib.rs:192-216`, `main.rs:1232-1257`). `mutation_for` đóng dấu
  impersonator (`lib.rs:33-51`).
- **Các thao tác ghi governance và transaction của chúng.**
  - Grant: `create_grant`, `patch_grant` → `write_grant` trên một transaction (`crates/egret-perm-compile/src/routes.rs:1531-1550`,
    `:1583-1595`, `:1047`); `delete_grant` (`:1612-1640`). `preview` chạy cùng `write_grant`/`write_rule` và luôn
    rollback (`:1938-1962`).
  - Rule: `create_rule`, `patch_rule`, `delete_rule` (`:1776`, `:1821`, `:1850`) → `write_rule` (`:1314`).
  - Compile thủ công: `compile` gọi `recompile_service` dựa trên pool, hàm này tự mở và commit transaction
    (`routes.rs:3731-3748`, `trigger.rs:1356-1365`).
  - Migrate và undo: mỗi cái một transaction; migrate chỉ commit khi đã chứng minh và không phải dry run
    (`routes.rs:3838-3871`, `:3888-3898`; `migrate.rs:840`, `:1458`).
  - Policy Ranger viết tay: tạo và sửa qua transaction của `write_policy_checked`
    (`crates/egret-policy/src/admin/mod.rs:1348-1427`, caller `:1757`, `:1774`, `:1848`, `:1866`), xoá (`:1885-1903`),
    bật/tắt (`:1926-1944`).
  - Clearance: `set_policy`/`clear_policy`, transaction riêng (`crates/egret-bi/src/tags.rs:514-586`), sau
    `tags.rs:285`/`:334` của control plane.
- **Compile.** Mọi lần compile, ở mọi crate, đều đến `recompile_locked_service_in_tx` (`trigger.rs:1332-1350`), qua
  `recompile_service_in_tx` (`:1321`) hoặc `recompile_services_in_tx` (`:1913`). `compile_desired` (`:373`) trả `None`
  cho loại profile mà compiler không nhắm tới; phễu khi đó trả summary rỗng. `CompileSummary` giữ
  `created/updated/deleted/unchanged/skipped` (`:255-262`). Dấu vết duy nhất được lưu hôm nay là
  `perm.compiled.compiled_at` theo từng policy đã compile (`migrations/0154_perm_model.sql:111-127`).
- **Tín hiệu.**
  - Ref không phân giải được có `failsOpen` (`principal_refs.rs:395-406`, `:447`), route `routes.rs:1687` (`policy:read`).
  - Overlap đã lưu (`overlap.rs:478`), route `routes.rs:1656` (`policy:read`). Đọc unit của mọi profile trong tiến
    trình; không gọi engine.
  - Cảnh báo policy theo profile, `classify::profile_warnings` (`classify.rs:912`), route `routes.rs:3799`
    (`policy:read`). Chưa có tổng trên mọi profile.
  - Lineage view thất bại: `view_lineage_state.state = 'failed'` (`migrations/0187_*.sql`), được `unknown_lineage`
    đọc (`crates/egret-controlplane/src/governance_lineage.rs:436-450`), route của nó là `policy:read` HOẶC `tag:read`
    (`governance_lineage.rs:169`).
  - Lỗi apply attachment catalog: `registry.engine_catalog_attachments.apply_status = 'failed'`, `apply_error`
    (`migrations/0147_catalog_registry.sql:54-55`), do `GET /catalogs` phục vụ dưới `catalog_registry:read`
    (`crates/egret-engine-mgmt/src/catalogs.rs:305`).
  - Engine: `engine_mgmt.engines.status ∈ provisioning|running|failed|deleting` (`migrations/0145_engine_status_check.sql:12-13`),
    `origin ∈ provisioned|external`, với `status` cố ý vô nghĩa cho dòng external (`migrations/0184_engine_external.sql:13-24`);
    `authz_service_id` gắn một profile (`0058`). Route engine cần `engine:read`.
  - Sổ download của plugin: `policy.ranger_plugin_downloads(service_id, plugin_id, kind, last_known_version,
    served_version, at)` (`migrations/0186_plugin_downloads.sql`); chỉ `last_known_version` là bằng chứng plugin đã
    áp dụng một version (`plugin_downloads.rs:7-10`). Bộ đếm: `authz_services.policy_version`, `tag_version`, và
    `updated_at`, được mọi lần tăng version đặt (`trigger.rs:1238-1240`, `crates/egret-policy/src/store.rs:2051`,
    `crates/egret-catalog-registry/src/tag_version.rs:117`) nhưng các update khác cũng có thể đặt.
  - Clearance: `classification_policies(tag_id, required_roles, required_groups)`; cả hai rỗng nghĩa là không có
    cổng (`egret-bi/src/tags.rs:507-509`); được `list_tags` liệt kê dưới `tag:read` (`controlplane/src/tags.rs:110-139`).
  - Catalog và engine của chúng: `GET /governance/catalogs` (`governance_browse.rs:124-150`), `policy:read` HOẶC
    `tag:read` qua `require_governance_read` (`governance_browse.rs:337-348`).
- **Frontend.** `GovernanceIndex` chuyển hướng tới `/governance/data` và cũng phục vụ catch-all
  (`frontend/src/App.tsx:529-531`, `:538`, `:606`). Data và People là khu lazy có bundle guard (`App.tsx:52`, `:61`).
  `OverlapReport` render ở cuối `/governance/policies` (`frontend/src/routes/governance/Policies.tsx:287`); cảnh báo
  theo profile ở `/governance/policies/:service` (`ProfileDetailPage.tsx:224`). Index của People nhận
  `?kind=unmatched` (`people/ByNameRedirect.tsx:59`). Trang engine và catalog là `/control-plane/engines/:id` và
  `/control-plane/catalogs/:id` (`App.tsx:483`, `:486`); `engine:read` và `catalog_registry:read` mỗi quyền đều cho
  vào Control Plane (`frontend/src/auth/appAccess.ts:14-22`).

## 2. Trang

`/governance` (index). Một trang theo khung `ListPageShell` (FE-PAGE-1): `h1` "Overview", rồi ba phần.

**Dải số đếm** (`SummaryStrip`): catalogs · egret grants · egret rules · classifications. Số mà người đọc không được
đọc thì bị bỏ đi, không bao giờ hiện là 0. Ghi "egret grants/rules" vì policy engine viết tay không được đếm (luật
trung thực của #1/#2: nhãn không bao giờ khẳng định nhiều hơn cái nó đếm).

**Needs attention.** Ba nhóm theo thứ tự Critical, Warning, Info; nhóm không có phát hiện thì không render. Mỗi
dòng: từ chỉ mức độ và icon (không bao giờ chỉ bằng màu, FE-A11Y-4), một câu có số đếm, tối đa năm mẫu mỗi mẫu có
link riêng, và link "Open" của dòng tới nơi sửa. Dưới các nhóm:
- `notChecked`: "Not checked for you: engine liveness and plugin drift (needs `engine:read`) …", mỗi nguồn bị bỏ
  qua một dòng;
- một dòng cố định "Not checked here": các view có thể làm lộ dữ liệu đã phân loại qua DEFINER (link tới
  Classifications), độ phủ classification.

Trạng thái rỗng (suy từ nội dung, FE-STATE-4): chỉ khi `findings` rỗng **và** không phát hiện nào là
`source_unavailable`, phần này mới hiện "Nothing needs attention.", kèm "among the checks you can run" khi `notChecked`
không rỗng. Vì vậy một response chỉ có `notChecked` không bao giờ tuyên bố mọi thứ ổn.

**Recent changes.** Một danh sách (FE-PAGE-4 `ui/table`, hẹp: mỗi dòng một card dưới 640 px): khi nào (tương đối, với
thời gian tuyệt đối trong `title` và trong `<time dateTime>`), ai ("Ana" hoặc "Admin as Ana" khi impersonate; "a
deleted user" khi dòng actor không còn), làm gì (câu do server dựng), và link tới object khi nó còn tồn tại (sự kiện
của một grant đã xoá vẫn gọi tên nó nhưng không link đi đâu). 25 mỗi trang, "Show older" dùng con trỏ keyset. Chú
thích: "Recorded since <ngày của sự kiện đầu tiên hoặc 'this version'>. Changes older than N days are pruned." (N lấy
từ `retentionDays`; "never pruned" khi null). "Role assignments and catalog tags are recorded after they commit and
can be missing if the audit writer was overloaded." Rỗng: "No governance change has been recorded yet. Recording
began when this version was installed; earlier changes are not listed."

Mỗi phần có trạng thái loading (skeleton), lỗi (thông điệp của server, nút thử lại) và rỗng riêng; một request lỗi
không bao giờ làm trống cả trang (FE-STATE-1/2/3).

**Link theo loại phát hiện** (đều dựng ở client từ ref có kiểu, §4.1):

| loại | Open | link của mẫu |
|---|---|---|
| `unresolved_principals_fail_open` | `/governance/people?kind=unmatched` | `/governance/grants/:id` hoặc `/governance/rules/:id` |
| `plugin_silent`, `plugin_behind`, `plugin_quiet` | `/governance/policies/:profile` | `/control-plane/engines/:id` |
| `compile_failed` | `/governance/policies/:profile` | như trên |
| `engine_failed` | `/control-plane/engines` | `/control-plane/engines/:id` |
| `rule_overlaps` | `/governance/policies` (báo cáo ở cuối trang) | trang rule khi thành viên là rule |
| `lineage_failed` | mẫu đầu tiên | trang object của Data, `?tab=lineage` |
| `catalog_apply_failed` | `/control-plane/catalogs` | `/control-plane/catalogs/:id` |
| `policy_warnings` | `/governance/policies` | `/governance/policies/:profile` |
| `plugin_unmodelled` | `/control-plane/engines` | `/control-plane/engines/:id` |
| `tags_without_clearance` | `/governance/classifications` | `/governance/classifications/:id` |
| `catalogs_without_engine` | `/governance/data` | trang catalog của Data |
| `source_unavailable` | không có | không có |

## 3. Backend: ghi nhận

### 3.1 Helper (F11 dùng lại được)

`crates/egret-audit/src/governance.rs`, export là `egret_audit::governance`. Tên hàm, chữ ký và các chuỗi action giữ
nguyên như bản tiếng Anh (khối code ở §3.1 của bản EN là chuẩn): `action::*` (`perm_grant:create|update|delete`,
`perm_rule:*`, `tag_clearance:set|clear`, `ranger_policy:create|update|delete|enable|disable`,
`policy_migration:apply|undo`, `profile:compile`; F11 thêm `engine_grant:create|update|delete`, họ Policy trong gói của
nó), `Family { Policy, Tag }`, `family_of(action, resource_type)`, `GOVERNANCE_ACTIONS`, `Change { action,
resource_type, resource_id, summary, principal }`, và `record_change(conn, change)` — INSERT một dòng `kind =
'mutation'` trên connection (transaction) của caller, đóng dấu `actor_user_id` và `impersonator` y như `mutation_for`
(`lib.rs:33-51`); summary trên 16 KiB được thay bằng `{"truncated": true, "bytes": n}`.

Vì sao ghi trong transaction mà không qua sink: feed là bằng chứng ai đã đổi governance; sink bỏ sự kiện khi tràn,
chạy sau commit, và tắt khi `audit.enabled = false` (tiền đề 1). Trong transaction, sự kiện tồn tại đúng khi thao tác
ghi của nó đã commit: một overlap 409, một `preview` (`routes.rs:1946-1959`), một migrate dry-run hay chưa chứng minh
(`routes.rs:3855-3862`) không để lại gì. Thao tác ghi governance có tần suất thấp, nên một INSERT thêm mỗi lần ghi
không tốn gì đo được. Doc comment của `audit.enabled` (`egret-config/src/lib.rs:193`) được cập nhật như bản EN.

Summary nhỏ và ổn định, không bao giờ là cả body:
- grant: `{catalog, schema, table, column, effect, actions, include[], exclude[]}` (chuỗi kind:ref), trước và sau;
- rule: `{name, kind, selector (path hoặc tag key), function, include/exempt[]}`;
- clearance: `{tag_key, required_roles[], required_groups[] (tên, phân giải lúc ghi)}`;
- policy Ranger: `{service, name, policy_type, enabled, resources (khoá và giá trị, mỗi danh sách tối đa 20)}`;
- migrate: `{service, policies[], created_grants[], created_rules[]}`; undo: `{service, policies[]}`;
- compile: `{service, created, updated, deleted, unchanged, skipped}`.

Mỗi before/after được đọc trong cùng transaction trước khi ghi. Thao tác ghi không có actor (sync workspace-assignment,
các sweep) không ghi gì (O13).

### 3.2 Mỗi sự kiện được ghi ở đâu

| thao tác ghi | INSERT đặt ở đâu | sự kiện |
|---|---|---|
| tạo/patch grant | `write_grant`, sau khi ghi dòng, trước recompile; `preview` cũng tới đây và rollback | `perm_grant:create`/`update` |
| xoá grant | `delete_grant`, sau `delete_grant_in_tx` (`crates/egret-perm/src/store.rs:504`) | `perm_grant:delete` |
| tạo/patch/xoá rule | `write_rule`; `delete_rule` | `perm_rule:*` |
| migrate | `migrate_service_policies`, chỉ khi `commit` là true, trước `tx.commit()` | `policy_migration:apply`, cộng một `perm_grant:create` / `perm_rule:create` cho mỗi đối tượng nó tạo (chúng đi qua `write_grant`/`write_rule`): 1 + N sự kiện |
| undo | `undo_service_policy_migration`, trước commit | `policy_migration:undo`, cộng một `perm_grant:delete` / `perm_rule:delete` cho mỗi đối tượng undo xoá, đối xứng với apply (review I2) |
| compile thủ công | `compile`, viết lại để tự mở transaction, gọi `recompile_service_in_tx`, ghi, commit | `profile:compile` |
| đặt/xoá clearance | `ClassificationPolicyRepo::set_policy`/`clear_policy`, trước commit; cả hai nhận thêm tham số `actor: &Principal` (thay `by: Option<Uuid>` của `set_policy`, giá trị là `actor.user_id`) | `tag_clearance:set`/`clear` |
| tạo/sửa policy Ranger | `write_policy_checked`, trước commit (nó nhận thêm principal) | `ranger_policy:create`/`update` |
| xoá/bật-tắt policy Ranger | `delete_policy_handler`, `set_policy_enabled`, trước commit | `ranger_policy:delete`/`enable`/`disable` |

### 3.3 `perm.compile_runs` (migration 0196)

Định nghĩa bảng như bản EN (`service_id` PK tham chiếu `policy.authz_services` `ON DELETE CASCADE`, `outcome ∈
ok|error`, `attempted_at`, `error`, `last_ok_at`, các số đếm, CHECK lỗi phải có lý do).

- **Thành công**: `recompile_locked_service_in_tx` upsert `outcome = 'ok'`, `attempted_at = last_ok_at = now()`, số
  đếm của summary, `error = NULL`, trên transaction của lần compile, khi `compile_desired` trả về một bản compile. Mọi
  đường compile ở mọi crate đều tới đúng hàm này (§1), nên không sót đường nào, và một lần compile mà caller rollback
  không để lại dòng (không có gì đổi trên engine). Profile đã bị khoá (`lock_services_in_tx`), nên các upsert cho một
  profile được tuần tự hoá.
- **Thất bại**: chỉ ghi ở nơi không có thao tác ghi nào bị từ chối cùng với nó, vì khi đó profile tụt sau model mà
  không có gì khác báo: `recompile_service` (điểm vào dựa trên pool; caller: route compile thủ công trước khi viết lại,
  và `ProfileCompilerImpl::recompile_profile`, `crates/egret-bin/src/main.rs:275-280`) và route compile thủ công đã viết
  lại. Sau khi transaction lỗi rollback, `record_compile_failure(pool, service_id, &error)` upsert `outcome = 'error'`,
  `attempted_at = clock_timestamp()`, `error` (2 000 ký tự đầu), giữ `last_ok_at` và số đếm,
  `WHERE compile_runs.attempted_at <= EXCLUDED.attempted_at`. Route compile thủ công cũng ghi `profile:compile` với
  `{outcome: "error", error}` trong cùng transaction đó.
- **Không ghi là thất bại**: lần compile thất bại bên trong một thao tác ghi grant, rule, tag, clearance, thành viên,
  policy hay catalog. Thao tác đó bị từ chối kèm lỗi (caller thấy), và engine giữ lần compile trước.
- Migration 0197: `CREATE INDEX audit_log_mutation_ts_idx ON audit.audit_log (ts DESC, id DESC) WHERE kind = 'mutation'`,
  để feed không quét các dòng decision.

### 3.4 `GET /api/control-plane/governance/changes`

`?limit=` (mặc định 25, tối đa 100) `&cursor=<ts_rfc3339>,<id>` → `{ entries, nextCursor, retentionDays }`.

- Gate: `require_governance_read` (`policy:read` HOẶC `tag:read`). Rồi các họ của người đọc: `Policy` cần
  `policy:read`, `Tag` cần `tag:read`. Các họ:
  - Policy: `perm_grant:*`, `perm_rule:*`, `ranger_policy:*`, `policy_migration:*`, `profile:compile`, và
    `role:assign`/`role:unassign` cũ với `resource_type = 'grant'` (người giữ role vốn đã đọc được dưới `policy:read`,
    #2 Q2);
  - Tag: `tag_clearance:*`, và `tag:assign`/`tag:unassign` cũ **chỉ với `resource_type = 'catalog'`** (các dòng
    `object` là tag BI trong workspace, tiền đề 8).
- SQL: `kind = 'mutation' AND (action, resource_type) IN (các cặp được phép) AND (ts, id) < cursor ORDER BY ts DESC,
  id DESC LIMIT n`, dùng index 0197. Danh sách cặp lấy từ `egret_audit::governance`, nơi duy nhất F11 thêm chuỗi của nó.
- Mỗi entry: `{ id, at, actor: {userId, label} | null, impersonator: {userId, label} | null, action, sentence,
  target: Ref | null, targetExists: bool, detail }`. Nhãn được phân giải lúc đọc: tên hoặc email user, tên role theo
  id (id khi role đã xoá), tên tag, tên catalog từ `registry.catalogs.warehouse_id` cho sự kiện tag catalog cũ, tên
  profile. `sentence` do server dựng, mỗi action một hàm, có unit test ("Ana created a grant: allow read on ice.s.t to
  group g1").
- `retentionDays`: `audit.retention_days` đã cấu hình khi dương, ngược lại `null`. `CpState` có thêm
  `audit_retention_days: Option<i64>`, đặt trong `egret-bin` từ cùng quyết định `scheduled_retention` mà việc dọn dùng
  (`main.rs:1232-1235`).

## 4. Backend: attention

### 4.1 Route và hình dạng

`GET /api/control-plane/governance/attention` trong `crates/egret-controlplane/src/governance_overview.rs` mới (cạnh
các route `/governance/*` khác, tag `control-plane-governance`), gate `require_governance_read`. Hình dạng response
(`counts`, `findings[] {kind, severity, count, sentence, sample[≤5] {label, detail, ref}, ref, source}`, `notChecked[]
{kind, requires}`) và kiểu `Ref` giữ nguyên như bản EN.

Phát hiện được sắp critical, warning, info, rồi theo kind. `count` là tổng thật; `sample` tối đa năm, thứ tự xác định
(mỗi nguồn nêu thứ tự của nó). Kiểm tra quyền gọi `egret_core::authz::require` và đổi `Forbidden` thành "không giữ";
mọi lỗi khác là 500 (PDP hỏng không phải là thiếu quyền).

### 4.2 Thực thi

Một transaction `REPEATABLE READ, READ ONLY` với `SET LOCAL statement_timeout = '5s'` (mẫu của `list_overlaps`,
`routes.rs:1662-1667`). Mỗi nguồn chạy trong savepoint riêng (`tx.begin()`), vì trong Postgres một câu lệnh lỗi sẽ
huỷ cả transaction: một nguồn lỗi hoặc quá hạn rollback savepoint của nó và cho ra
`{kind: "source_unavailable", severity: "warning", source: <kind>, sentence: "Could not check <what>."}` (lỗi được ghi log, không hiện ra); các
nguồn khác vẫn chạy tiếp. Nguồn gọi hàm store trên transaction, không bao giờ gọi route. Sổ download plugin, engine và
profile được đọc trong cùng snapshot, nên độ lệch được xét trên một trạng thái.

### 4.3 Các nguồn

Bảng nguồn (kind, mức độ, quyền, luật) giữ nguyên như bản EN §4.3. Tóm tắt:
- critical: `unresolved_principals_fail_open` (`policy:read`); `plugin_silent`, `plugin_behind` (`policy:read` +
  `engine:read`); `compile_failed` (`policy:read`); `engine_failed` (`engine:read`, chỉ `origin = 'provisioned'`);
- warning: `plugin_quiet`; `rule_overlaps` (`policy:read`); `lineage_failed` (`policy:read` HOẶC `tag:read`);
  `catalog_apply_failed` (`catalog_registry:read`); `policy_warnings` (`policy:read`, cộng trên mọi profile);
  `plugin_unmodelled` (`engine:read`);
- info: `tags_without_clearance` (`tag:read`); `catalogs_without_engine` (`policy:read` HOẶC `tag:read`);
- counts: catalogs theo quyền đọc governance; grants, rules theo `policy:read`; tags theo `tag:read`.

Luật độ lệch: chỉ xét profile được gắn với ít nhất một engine `origin = 'provisioned' AND status = 'running'`.
`plugin_silent`: không có dòng `policies` nào với `at >= now() - 24 h`. `plugin_behind`: với mỗi kind có dòng, lấy bộ
đếm V (policy_version hoặc tag_version) và `updated_at` U; nếu `U < now() - 90 s` thì chưa xác nhận khi không dòng nào
poll lúc hoặc sau U, hoặc có dòng poll lúc hoặc sau U báo `last_known_version < V`. Dòng poll trước U (plugin id của
pod cũ) bị bỏ qua. `plugin_quiet`: lần poll cuối cũ hơn 5 phút nhưng trong 24 h.

Các câu mang sự trung thực mà chương trình đã học ở #1 và #2: nhãn không bao giờ khẳng định nhiều hơn dữ liệu cho thấy.
- `plugin_silent`: "No plugin has polled profile P in 24 hours, while engine E is recorded as running. egret cannot
  tell a stopped engine from one that cannot reach egret or does not poll."
- `plugin_behind`: "No plugin of profile P has reported holding policy version V, set at T." Không bao giờ nói plugin
  "enforce" điều gì: sổ ghi cái plugin báo, và một poll giả mạo có thể xác nhận sớm (`plugin_downloads.rs:12-14`).
- `plugin_quiet`: "No plugin has polled profile P for N minutes."
- Engine external không bao giờ bị xét liveness hay độ lệch; khi profile cũng gắn một engine external, detail của phát
  hiện nói: "egret does not run external engines and cannot tell whether they should be polling" (`0184:13`).
- **Được làm rõ bởi phép đo sống (Task 1, `docs/en/plans/2026-10-18-dg-overview-live.md` §1.1):**
  - Liveness (`plugin_quiet`, `plugin_silent`) đọc lần poll `policies` **mới nhất** của mỗi profile. Plugin id của
    một pod bị lập lịch lại giữ nguyên `at` cuối trong 24 h; luật "có dòng nào cũ hơn N" sẽ báo sai `plugin_quiet`
    mỗi lần lập lịch lại. Liveness chỉ đọc `policies`: plugin nào cũng poll nó.
  - `plugin_behind` chỉ đọc các dòng poll lúc hoặc sau U **và** trong 5 phút (id của pod đã chết không là bằng chứng
    cũng không bị quy lỗi), và chỉ với profile chưa quiet/silent (liveness đã báo). Báo cáo
    `lastKnownVersion = -1` là "chưa báo": không xác nhận và cũng không tính là chậm. Bộ lấy tag của Trino báo `-1`
    trong một lần poll tag sau mỗi lần tải policy, và plugin nào cũng báo vậy ở lần poll đầu sau khi khởi động lại.
    Cái giá được chấp nhận (quyết định (b) của brief Task 7): plugin vẫn poll nhưng không bao giờ báo version nào
    thì không luật độ lệch nào đánh dấu; `plugin_quiet` và `plugin_silent` chỉ bắt được plugin khi nó ngừng poll. Ngưỡng vẫn 90 s cho cả
    hai kind (xác nhận cần hai lần poll, khoảng 60 s ở poll 30 s mặc định).
  - Một kind chỉ được xét khi profile có dòng của kind đó poll trong 24 h. StarRocks không bao giờ tải tag, nên
    không bao giờ báo thiếu lần tải tag cho nó.
- `updated_at` cũng bị các update khác đặt, điều đó chỉ có thể làm `plugin_behind` đến muộn, không bao giờ báo sai;
  spec nói vậy và Task 1 đo điều đó.
- `tags_without_clearance`: "N classifications have no clearance: they gate nothing." Không nói "unprotected".

### 4.4 `ranger_plugin_report` chuyển sang `egret-perm`

`plugin_report` (`engines.rs:335-371`) chỉ phụ thuộc các hàm của `egret_perm` (`ranger_plugin_is_modelled`,
`plugin_version_for_image`, `stored_engine_image`, `engine_images::known_image_refs`,
`modelled_ranger_plugins_sentence`) và `is_external`. Nó chuyển thành `egret_perm::ranger_plugin_report(engine_type,
origin, declared_version, spec) -> PluginReport { version, modelled, note }`; `EngineDto::from` gọi nó và các test của
nó (`engines.rs:7194-7305`) vẫn xanh không đổi. `egret-controlplane` vốn đã phụ thuộc `egret-perm`, nên không thêm
cạnh crate nào (thêm `egret-engine-mgmt` sẽ là một cạnh mới).

## 5. Frontend

- `App.tsx`: `const OverviewRoutes = lazy(() => import("@/routes/governance/overview/OverviewPage"))`; route index
  render nó trong `LazyRouteArea` dưới `RequirePermission perm={["policy:read","tag:read"]}`; bỏ `GovernanceIndex`; `*`
  thành `<Navigate to="/governance" replace />` (tuyệt đối, theo ghi chú vòng lặp react-router 7 ở `App.tsx:526-528`).
  URL governance không biết trước đây rơi vào Data; giờ rơi vào Overview.
- Sidebar: nhóm đầu tiên `{ heading: "Governance overview", items: [{ label: "Overview", icon: Gauge, perm:
  ["policy:read","tag:read"], to: "", end: true }] }`. `NavItem` nhận thêm `end?: boolean` (thêm, không đổi cái cũ)
  mà `renderItem` truyền cho cả hai `NavLink` (`Sidebar.tsx:230-254`); không có nó mục này sẽ active trên mọi trang
  governance vì đường dẫn của nó là base của khu. Comment giữ chỗ được thay.
- File: `routes/governance/overview/{OverviewPage,CountsStrip,AttentionSection,AttentionRow,RecentChanges}.tsx`,
  `overview/refUrl.ts` (Ref → URL, dùng `peopleUrl.ts` và `data/objectPath.ts`, cả hai import tĩnh được),
  `overview/queryKeys.ts` chỉ khi cần key viết tay (FE-DATA-2), `__tests__/`. Bundle guard
  `frontend/scripts/assert-governance-overview-lazy.mjs`, script `test:bundle:governance-overview`, liệt kê trong
  FE-ROUTE-2 và FE-TEST-4 của `docs/en/frontend-rules.md`.
- Sinh lại client: `cargo build -p egret-bin && target/debug/egret openapi > openapi.json`, rồi
  `cd frontend && npm run gen:api` (không bao giờ `orval` trần).
- `docs/en/frontend-rules.md` đầy đủ; ngôn ngữ thiết kế Metabase và token OKLCH; `ui-ux-pro-max` cho chi tiết thị giác;
  390 px; jsx-a11y. FE-REF: không có sản phẩm nguồn; Overview theo khung list-page của egret, một lựa chọn có chủ ý
  được ghi ở đây.

## 6. Kiểm thử

- **Ghi nhận** (test DB trên DB test lấy từ `egret_test_db::url()`, fixture giới hạn trong test): mỗi dòng của §3.2
  ghi đúng một sự kiện với action, resource, actor, impersonator và before/after đúng; thao tác bị từ chối (overlap
  409), `preview`, dry-run và migrate chưa chứng minh không ghi gì; sự kiện vẫn được ghi khi `audit.enabled = false`
  (không sink nào được rút); summary trên 16 KiB bị cắt.
- **compile_runs**: thành công qua mỗi điểm vào phễu ghi `ok` với số đếm; thao tác ghi có compile lỗi không ghi gì;
  `recompile_service` lỗi ghi `error` và giữ `last_ok_at`; thành công sau đó xoá `error`; loại profile không được nhắm
  tới không ghi gì.
- **attention**: mỗi kind bật trên một fixture tối thiểu và im trên cặp phủ định của nó (ví dụ dòng plugin cũ hơn
  `updated_at` của version bị bỏ qua; engine external không bao giờ bị xét); nguồn lỗi cho ra `source_unavailable`
  trong khi các nguồn khác vẫn trả lời. Các nguồn là một danh sách `(kind, permission, fn)` mà `attention_via` chạy;
  test truyền danh sách thật cộng một nguồn chạy `SELECT 1/0` trên transaction, đặt nó đầu tiên, để nếu thiếu
  savepoint thì mọi nguồn sau nó đều hỏng; lọc theo quyền với `CedarPdp` thật: steward giữ `policy:read` mà không có
  `engine:read` không nhận phát hiện plugin hay engine nào và thấy chúng trong `notChecked`; chỉ `tag:read` thấy phát
  hiện tag, lineage và catalog; 403 khi không có quyền nào.
- **changes**: lọc theo họ (người đọc `tag:read` không bao giờ thấy sự kiện grant; sự kiện tag `object` của BI không
  bao giờ được trả), phân trang bằng con trỏ không trùng, nhãn cho actor và role đã xoá, `retentionDays`.
- **Frontend vitest**: route và chuyển hướng (`/governance` và `/governance/nope`), thứ tự và gate của sidebar; số đếm
  bị bỏ theo quyền; câu và link của mỗi loại phát hiện; nhóm theo mức độ; trạng thái rỗng chỉ khi nội dung cho phép,
  kể cả trường hợp chỉ có `notChecked` và có `source_unavailable`; phân trang Recent changes, impersonation, target đã
  xoá, trạng thái rỗng và lỗi.
- **Live** (bắt buộc): Task 1 của plan đo sổ download plugin trước khi code các luật độ lệch; bước kiểm tra bằng trình
  duyệt seed từng phát hiện và chụp màn hình ở 390 px và desktop.
