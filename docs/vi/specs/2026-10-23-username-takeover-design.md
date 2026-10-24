# Chiếm username sau khi xoá user: thiết kế

> **Trạng thái:** Bản tham khảo tiếng Việt · **Ngày:** 2026-10-23
> ⚠️ Bản gốc (canonical) là tiếng Anh: [`docs/en/specs/2026-10-23-username-takeover-design.md`](../../en/specs/2026-10-23-username-takeover-design.md). Khác biệt thì ưu tiên bản tiếng Anh.

Trạng thái: đã triển khai (plan Task 1–8, 2026-10-23), đã live check trên stack riêng 2026-10-24; Platform docs theo PR riêng (D5). Base: `origin/main` `aec59807`. Branch `fix/username-takeover`.
Plan: [`../../en/plans/2026-10-23-username-takeover.md`](../../en/plans/2026-10-23-username-takeover.md).
Chương trình: KI fix programme, gói security. Là follow-up của review #692 (một trong tám follow-up; bảy cái còn
lại là non-goal, §11).

User đã chốt phạm vi ngày 2026-10-23 (§3, D1–D5). Tài liệu này hiện thực các quyết định đó và ghi lại những lựa
chọn mà code buộc phải có (§4). Các câu hỏi mở ở §12 đã được trả lời (D4, D5).

## 1. Vấn đề

egret lưu một số tham chiếu tới user bằng **username**, không phải bằng id:

- các dòng kind `user` của `perm.grant_principals`, `perm.rule_exemptions` và `perm.engine_grant_principals`;
- `users[]` của các Ranger policy viết tay (`managed_by = 'operator'`);
- các infra policy `egret-admin - …`, cấp quyền ALL trên một engine cho mọi platform admin.

Engine so sánh tên trong policy với `preferred_username` trong engine JWT, từng byte một.

`UserRepo::hard_delete` xoá dòng `users` và vì thế giải phóng `username` của nó. `ensure_username_via` mint tên
**trống nhỏ nhất** trong `base`, `base-2`, … cho người tiếp theo có email slug ra cùng base. Người đó khi ấy thừa
hưởng mọi allow, mọi deny exemption và mọi mask hay row-filter exemption vẫn còn ghi tên ấy. Nếu người bị xoá là
platform admin, người tiếp theo còn thừa hưởng quyền ALL trên engine, vì policy `egret-admin` là một snapshot chỉ
được ghi khi engine apply và không bao giờ được ghi lại khi danh sách admin thay đổi.

Có ba lỗ hổng nhỏ hơn:

- `hard_delete` không recompile các profile mà user chạm tới qua **role**, **workspace membership** hay
  **clearance** của họ. Các body đã compile đó vẫn ghi username đã được giải phóng cho tới khi có thứ khác
  recompile chúng.
- Không có gì ghi lại ai đã bị xoá. Xoá user không ghi mutation event nào, nên audit về sau không thể nêu tên họ.
- Engine JWT mint trước khi xoá vẫn hợp lệ cho tới khi hết hạn.

## 2. Bằng chứng (đã kiểm tra tại `aec59807`)

Mọi dòng đều được đọc lại trong worktree. Dòng đánh dấu **new** là không có trong kết quả điều tra ban đầu, hoặc
sửa lại kết quả đó.

| id | sự kiện | ở đâu |
|---|---|---|
| E1 | `users.username` là `citext UNIQUE`, NULLable. Không đường insert nào set nó | `migrations/0040_user_username.sql:1` |
| E2 | Writer runtime duy nhất của `users.username` là `ensure_username_via`: một `UPDATE … SET username = $2 WHERE id = $1 AND username IS NULL` cho mỗi ứng viên bên trong một SAVEPOINT. Vi phạm unique thì rollback về savepoint và thử suffix tiếp theo. Tên duy nhất bị bỏ qua là `egret-lineage` | `crates/egret-auth/src/users.rs:429-508`, `:470`, `:466` |
| E3 | Các writer khác của `users.username` là migration: backfill của 0040 và `UPDATE users SET username = NULL WHERE username = 'egret-lineage'` của `0187` (cùng CHECK của nó). Seed (`deploy/demo/seed.sh`, `scripts/`) không bao giờ ghi nó. Test ghi trực tiếp bằng `INSERT INTO users (…, username)` trong khoảng 30 file test | `migrations/0040_user_username.sql:22`, `migrations/0187_view_lineage.sql:51-52`; ví dụ `crates/egret-perm-compile/src/principal_refs.rs:761` |
| E4 | Các caller mint: query đầu tiên (`crates/egret-query/src/execution/mod.rs:67`, `:87`); engine JWT (`crates/egret-bin/src/engine_minter.rs:74`); BI (`crates/egret-bi/src/routes.rs:2374`); alert owner (`crates/egret-automation/src/evaluator.rs:160`); engine admin seed (`crates/egret-engine-mgmt/src/engines.rs:2507`); compile (`crates/egret-perm-compile/src/members.rs:332`, `:359` `premint_model_usernames`); Governance allocate (`crates/egret-perm-compile/src/routes/people.rs:574`). Chỉ route Governance kiểm tra ref cũ (409, `:577-593`) | — |
| E5 | `hard_delete` lock các group của user và recompile profile của group cùng profile của các engine grant nhắm tới user. Nó xoá `sessions`, `user_groups` và `access_grants`, rồi tới dòng user (`RETURNING name, email`). Nó không tính các nhánh role, workspace hay clearance | `crates/egret-auth/src/users.rs:298-382`; `crates/egret-perm-compile/src/group_membership.rs:124-150`; `crates/egret-perm-compile/src/trigger.rs:1998-2033` |
| E6 | Các nhánh đã có sẵn mà `hard_delete` bỏ qua: `affected_services_for_role_membership` (clearance, grant, engine-grant, và impersonation khi role là `platform_admin`) và `affected_services_for_workspace_members`. Role holder và workspace member được đọc từ `access_grants` join với `users` | `trigger.rs:1726-1776`, `:1797-1860`; `members.rs:501-555` |
| E7 | Policy `egret-admin - <chain>` (`managed_by = 'infra'`, mỗi resource hierarchy một cái, `policyItems[0].users` = admin, `delegateAdmin: true`) chỉ được ghi bởi `seed_admin_all_access`, gọi từ đường engine apply của Trino và StarRocks với `admin_usernames()` | `crates/egret-policy/src/default_policies.rs:53-110`, `:167-197`; `crates/egret-engine-mgmt/src/engines.rs:771-776`, `:3821-3826`, `:2499-2512` |
| E8 **new** | `seed_admin_all_access` trả `Ok(0)` mà không ghi gì khi `users` rỗng, nên một tập co về rỗng không bao giờ xoá được body cũ | `default_policies.rs:173-175` |
| E9 **new** | `admin_usernames` → `RoleRepo::platform_admin_user_ids`, nhánh trực tiếp và nhánh group, **bỏ qua status**. Hiện nay admin bị disable vẫn nằm trong body `egret-admin`. `guard_not_last_admin` dùng cùng danh sách này, dù doc của nó nói "last active platform admin" | `crates/egret-auth/src/roles.rs:176-201`; `crates/egret-controlplane/src/access/mod.rs:85-111` |
| E10 **new** | Các writer của platform-role membership: `assign_user_role`/`unassign_user_role` (recompile qua `with_recompile`, `access/mod.rs:444`, `:498`); `PUT /users/{id}/platform-roles` (`replace_platform_roles`, có recompile, `crates/egret-workspace/src/rbac.rs:77-160`); **`assign_group_role`/`unassign_group_role`, gọi `RoleRepo::assign`/`unassign` không recompile** (`access/mod.rs:852`, `:892`); lệnh `INSERT INTO access_grants` thô của CLI `create-admin` (`crates/egret-bin/src/main.rs:133-145`); và xoá role, vốn bị từ chối với role seed sẵn như `platform_admin` (`access/mod.rs:1176-1181`) | — |
| E11 | Mọi writer của group membership đều đi qua `GroupMembershipRecompiler::lock_affected_in_tx` + `recompile_in_tx`: OIDC sync, thêm/bớt member, xoá group, và biến thể riêng của `hard_delete` | `crates/egret-auth/src/groups.rs:623-720`, `:897-950`, `:962-1065` |
| E12 | Mọi recompile đều dồn qua `recompile_locked_service_in_tx`, hàm này return sớm khi `compile_desired` là `None` (profile mà compiler không nhắm tới) | `trigger.rs:1439-1471`, `:2170-2201` |
| E13 | Status của user chỉ được ghi bởi `UserRepo::set_status`, từ `PATCH /users/{id}` (disable thu hồi session; không có recompile) | `crates/egret-auth/src/users.rs:213-221`; `access/mod.rs:294-309` |
| E14 | `DELETE /users/{id}` không ghi event nào. `service_account:delete` ghi `{name, email}` qua sink best-effort, không ghi username | `access/mod.rs:356-378`; `crates/egret-dataapi/src/routes.rs:679-711` |
| E15 | `record_change` insert một dòng `kind = 'mutation'` trên transaction của caller. Nó `debug_assert!` rằng cặp nằm trong `FAMILIES`. Nó ghi cả khi `audit.enabled` là false (quyết định O3). Mọi dòng audit bị prune sau `retention_days` (mặc định 90) | `crates/egret-audit/src/governance.rs:63-136`, `:194-240`; `crates/egret-audit/src/lib.rs:223-228` |
| E16 | Email service-account `svc.{slug}@{ws}.service.egret.local` slug thành `svc-{slug}`, cùng namespace với người thật | `crates/egret-dataapi/src/routes.rs:551` |
| E17 **new** | Engine JWT: `sub` = `preferred_username` = username, TTL **900 s**, không có claim `cgen`. Token exchange của machine-IdP yêu cầu `cgen`, nên engine JWT của user không refresh được. Token chỉ được mint phía server (query, BI, automation). User đã bị xoá không lấy được token mới: `ensure_username` trả `NotFound` | `crates/egret-bin/src/engine_minter.rs:10-27`, `:60-96`; `crates/egret-bin/src/main.rs:1650`; `crates/egret-machine-idp/src/lib.rs:433` |
| E18 **new** | `egret_delegator` (service user của impersonate-mode) **không** phải username reserved, nên một người thật có thể được mint tên này. Việc này đã được ghi nhận là một fix riêng (engine-grants spec E21) | `crates/egret-core/src/engine_principals.rs:18-29` |
| E19 | Mô hình attention của Governance: `SOURCES` với `Requires` trên một tập `held` gồm các chuỗi permission, mỗi cái một savepoint, các `Ref` có kiểu mà client chuyển thành URL | `crates/egret-controlplane/src/governance_overview.rs:174-300`, `:331-410`; `frontend/src/routes/governance/overview/refUrl.ts` |
| E20 | Các dialog xoá: `UsersPage.tsx:504-516` và `UserDetail.tsx:340-350`. Mô tả ghi "This user, their sessions, group memberships, and role grants will be permanently removed." `ConfirmDeleteDialog` nhận `children`. Dialog disable là `UserDetail.tsx:318-338`. `ControlPlaneUserDto` không có `username` | `frontend/src/routes/control-plane/access/*`; `frontend/src/components/ui/confirm-delete-dialog.tsx:61` |
| E21 | Số migration: số cao nhất trên `origin/main` là `0206_workspace_invites.sql`. Một branch đang mở khác lấy 0207. **Gói này giữ chỗ `0209`** (trước đó giữ `0208`, đến khi PR đang mở #740 lấy số đó; xem §13, "Đánh số lại thành 0209") | `migrations/` |

## 3. Quyết định của user (chốt 2026-10-23, không mở lại)

- **D1.** Username không bao giờ được dùng lại. Một ledger append-only bảo đảm điều đó cho mọi đường mint, cả người
  thật lẫn service account. Migration seed ledger với mọi tên có thể biết được. UI và API xoá hướng admin sang
  Disable.
- **D2.** `egret-admin` đi theo platform_admin membership. Mọi thay đổi ghi lại các policy `egret-admin` của mọi
  engine bị ảnh hưởng **trong cùng transaction**. Một boot pass sửa các body cũ.
- **D3.** Các vụ chiếm tên trong quá khứ **chỉ được báo cáo**: một route chỉ dành cho admin và một attention signal
  ở Overview. Không có gì bị disable tự động.
- Cũng trong phạm vi: lấp các lỗ recompile của `hard_delete`; phát `user:delete` qua `record_change`; quyết định
  câu hỏi về JWT.
- **D4** (trả lời Q1). Event `user:delete` là một event family `User` riêng, đọc được bằng `user:read`, không phải
  `policy:read`, vì chúng mang email của người bị xoá (PII). §4.4 nói feed gate nó thế nào.
- **D5.** Platform docs (`egret-data/docs`) được cập nhật trong một docs PR riêng sau khi code PR merge, do
  controller làm, không nằm trong PR này. `docs/vi/specs/2026-10-23-username-takeover-design.md` được viết như bản
  dịch tham khảo tiếng Việt của spec này khi code xong (bản tiếng Anh là canonical).

## 4. Thiết kế

### 4.1 Ledger (D1)

**Bảng** (`migrations/0209_username_ledger.sql`, schema public, cạnh `users`):

```sql
CREATE TABLE username_ledger (
    username   citext PRIMARY KEY,
    user_id    uuid NULL REFERENCES users(id) ON DELETE SET NULL, -- current holder
    minted_for uuid NULL,        -- the user it was minted for; no FK, survives the delete
    minted_at  timestamptz NULL, -- NULL = held before 0209 (unknown)
    retired_at timestamptz NULL,
    source     text NOT NULL CHECK (source IN
        ('mint', 'backfill_user', 'backfill_ref', 'backfill_policy', 'backfill_audit', 'reserved'))
);
```

**Được bảo đảm bằng trigger trên `users`, không bằng code Rust.** Đây là lựa chọn thiết kế chính. Dòng ledger được
ghi bởi một trigger `AFTER INSERT` và một trigger `AFTER UPDATE OF username` trên `users`. Mỗi trigger chạy khi
`NEW.username` khác NULL và khác `OLD.username`. Hàm trigger:

1. return ngay nếu dòng ledger của `NEW.username` đã có `user_id = NEW.id` (holder set lại chính tên của mình);
2. nếu không thì chạy `INSERT INTO username_ledger (username, user_id, minted_for, minted_at, source) VALUES
   (NEW.username, NEW.id, NEW.id, now(), 'mint')`. Nếu tên đó từng được giữ, backfill hoặc reserved, lệnh này raise
   `23505` trên `username_ledger_pkey`.

Vì sao dùng trigger:

- **Cùng một savepoint.** Trigger chạy bên trong `UPDATE` của `ensure_username_via`, nên lệnh insert diễn ra trong
  mint savepoint sẵn có, đúng như D1 yêu cầu. `is_unique_violation()` khớp `23505` từ cả hai index. Lệnh `continue`
  sẵn có khi đó thử `base-2`. `ensure_username_via` không cần đổi logic (chỉ đổi docs).
- **Không có race check-then-insert.** Một `hard_delete` đồng thời của `bob` không thể mở ra khe hở nào: dòng
  ledger của `bob` đã commit từ khi `bob` được mint, và việc xoá chỉ set `user_id` về NULL. Một lần mint `bob` sẽ
  block trên unique index của `users` cho tới khi việc xoá commit, rồi fail ở ledger PK. Thiết kế kiểu "SELECT từ
  một bảng retired, rồi UPDATE" sẽ race dưới READ COMMITTED; thiết kế này thì không thể.
- **Nó phủ mọi writer, kể cả những cái không ai nhớ tới.** Tức là E2–E4, các test ghi `username` trực tiếp (E3),
  một seed tương lai, và `psql` của operator. Điều này theo bài học "unrepresentable beats unforgettable".

**Retire.** Một trigger `BEFORE DELETE ON users` set `retired_at = now()` trên các dòng ledger của user. Sau đó
`ON DELETE SET NULL` của FK xoá `user_id`. Một `UPDATE OF username` đổi hoặc NULL một tên (dạng của 0187) cũng
retire dòng cũ theo cách đó. `minted_for` không bao giờ bị xoá, nên ledger ghi vĩnh viễn ai đã giữ tên. Bản ghi
này tồn tại lâu hơn audit retention (E15).

**Append-only.** Một trigger `BEFORE DELETE OR UPDATE ON username_ledger` từ chối `DELETE` và từ chối mọi thay đổi
`username`, `minted_for`, `minted_at` hay `source`. Chỉ `user_id` (hành động của FK) và `retired_at` được phép
đổi. Không API nào giải phóng một tên. Operator buộc phải giải phóng tên thì phải drop trigger trong một migration
thủ công, một cách có chủ đích.

**Seed** (bên trong 0209, dưới dạng hàm SQL `username_ledger_backfill()` mà migration gọi một lần). Mỗi bước là
`ON CONFLICT DO NOTHING`. Hàm vẫn gọi được, nên test có thể insert các dạng trước 0209 rồi chạy lại:

| bước | source | dòng | `retired_at` |
|---|---|---|---|
| 1 | `backfill_user` | mọi `users.username` (`user_id` = `minted_for` = user đó, `minted_at` NULL) | NULL |
| 2 | `backfill_ref` | mọi ref `kind = 'user'` trong ba bảng principal `perm.*` mà không `users.username` nào mang (citext-distinct) | `now()` |
| 3 | `backfill_policy` | mọi tên `users[]` trong bất kỳ item list nào của **bất kỳ** body `policy.authz_policies` nào (`operator`, `compiler` **và** `infra`), trừ macro `{…}`, mà không `users.username` nào mang | `now()` |
| 4 | `backfill_audit` | (a) từ các event `service_account:delete`, phần local của email được slug y hệt `slugify_localpart` (base slug; suffix thì không biết được); (b) mọi chuỗi `"user:<name>"` trong `before`/`after` của governance mutation event, không có holder hiện tại | `now()` |
| 5 | `reserved` | `egret-lineage`, `egret_delegator` | `now()` |

Bước 3 rộng hơn câu chữ của D1 ("hand-written"), và có chủ đích. Một body **compiled** cũ (lỗ hổng E6) hay một body
**`egret-admin`** cũ (E7) ghi một tên đã giải phóng nguy hiểm y như body viết tay, và `egret-admin` là trường hợp
tệ nhất. Bước 3 cũng giữ `egret_delegator` ở bất cứ đâu có impersonate engine. Bước 5 sau đó đóng E18 cho mọi bản
cài đặt, với giá một dòng. `is_reserved_engine_username` và CHECK của 0187 giữ nguyên.

SQL slug ở bước 4a phải khớp với `slugify_localpart` (lowercase phần local; thay mỗi chuỗi ký tự ngoài
`[a-z0-9_]` bằng một `-`; trim `-`; rỗng thì thành `user`). Một test so sánh hai bên trên một corpus.

**Cái giá của ledger.**

- Một người bị xoá rồi quay lại (OIDC re-login tạo dòng mới, `users.rs:182`; admin tạo lại tài khoản local) nhận
  `bob-2`. Các allow ghi `bob` không còn áp dụng cho họ, tức fail closed. **Các deny, exclusion và mask exemption
  ghi `bob` cũng không còn ràng buộc họ, tức fail open đối với đúng người đó.** Báo cáo (§4.5) liệt kê các ref đó
  là holderless. Đây là lý do UI hướng admin sang Disable (§4.4).
- Một tên được viết sẵn trong policy viết tay cho một người chưa từng tồn tại sẽ bị bước 3 retire. Nếu người đó
  gia nhập sau này, họ nhận `name-2`.
- Service account được tạo lại nhận `svc-x-2`. Điều đó vô hại: client xác thực bằng OAuth client id.
- Ledger chỉ có tăng (mỗi tên từng được mint một dòng). Giới hạn 999 suffix của `ensure_username_via` không đổi.

**Guard allocate của Governance được giữ lại** (`people.rs:577-593`). Sau khi seed, ref holderless ghi một dòng
ledger và bị mọi lần mint bỏ qua. Tuy vậy, một ref insert bằng SQL thô sau 0209 vẫn bỏ qua ledger, và 409 vẫn bắt
được nó. Các test của nó (`tests/allocate_username.rs`) insert đúng dạng đó và vẫn pass.

### 4.2 `egret-admin` đi theo platform_admin (D2)

**Một định nghĩa duy nhất cho tập này.** `egret_auth::roles::engine_admin_user_ids_via(conn)` trả về các holder
platform_admin qua nhánh trực tiếp và nhánh group (hai nhánh của E9), **chỉ `status = 'active'`**, tăng dần theo
id. `admin_usernames` (engine apply) và phần sync trong transaction bên dưới đều gọi nó. Username được mint dưới
`lock_compile_mints_in_tx` (thứ tự mint của compile) và được sort, nên body là tất định. Danh sách sắp theo
`UNION` hiện nay có thể làm body bị churn.

**Admin bị disable bị loại khỏi `egret-admin`.** Lý do:

1. Policy mang nghĩa "được làm mọi thứ trên engine này". Tài khoản bị disable không được làm gì trong egret.
2. `egret-admin` là body duy nhất có blast radius là cả engine. Loại admin bị disable khỏi nó đóng con đường duy
   nhất mà JWT đã mint của admin bị disable (≤ 900 s, E17) hoặc một alert lên lịch chạy dưới danh nghĩa họ
   (`evaluator.rs:147-180` mint cho owner mà không kiểm tra status) giữ được quyền ALL.
3. Cái giá là thêm một điểm trigger: thay đổi status của một admin sẽ sync các body.

Phần còn lại của model đã compile vẫn ghi user bị disable (expansion theo role, group và workspace không lọc
status). Điều đó nhất quán với "Disable keeps memberships and grants" (chính lời của dialog disable), và không bị
thay đổi ở đây. Kiểm tra near-root của impersonation `is_platform_admin` vẫn không quan tâm status, đó là hướng an
toàn: nó từ chối nhiều target hơn, không bao giờ ít hơn.

**Writer.** `egret_policy::default_policies::sync_admin_all_access_in_tx(conn, service_id, users)`:

- đọc tên và type của service;
- dựng các body bằng `build_default_policies` (`users` có thể rỗng);
- upsert từng hierarchy với `ON CONFLICT … DO UPDATE … WHERE authz_policies.policy_jsonb IS DISTINCT FROM
  EXCLUDED.policy_jsonb`;
- tăng `authz_services.policy_version` chỉ khi có dòng thay đổi;
- trả về các tên đã bỏ và đã thêm.

Tập rỗng ghi `users: []`, một item không khớp ai, nên body cũ bị xoá (sửa E8). Những service nào:
`admin_policy_services_via(conn)`, mọi `service_id` giữ ít nhất một policy `managed_by = 'infra'` có tên thoả
`is_seeded_hierarchy_name(_, "egret-admin")`. Sync chỉ ghi lại các engine đã được seed. Engine apply vẫn tạo body
khi provision lần đầu: `seed_admin_all_access` trở thành một lời gọi tới cùng writer đó và giữ "create on apply",
nhưng không còn return sớm khi danh sách rỗng (khi đó nó ghi lại các dòng đang có và không tạo dòng nào).

**Chokepoint là recompile.** `recompile_locked_service_in_tx` (E12) gọi
`sync_admin_all_access_in_tx(service_id, current set)` **trước** `compile_desired`, khi service nằm trong
`admin_policy_services_via`. Nó làm việc này trước lần return sớm, vì một engine profile mà compiler không nhắm tới
vẫn mang `egret-admin`. Nó cũng đứng trước compile, để kiểm tra near-root của impersonation đọc được body mới. Vì
vậy mọi đường recompile hiện có đều hội tụ `egret-admin` cho các service mà nó lock. Các đường thay đổi admin chỉ
cần **thêm các admin-policy service vào tập mà chúng lock**:

| thay đổi | đường | cách các service vào tập bị lock |
|---|---|---|
| grant/revoke role của user | `assign/unassign_platform_role_and_recompile`, `replace_platform_roles` | `affected_services_for_role_membership("platform_admin")` có thêm nhánh admin-policy, cạnh nhánh impersonation |
| grant/revoke role của group | `assign_group_role`/`unassign_group_role` | **đổi** sang gọi `assign/unassign_platform_role_and_recompile(pool, "group", …)` (hiện nay chúng không recompile gì, E10). Mọi role, không chỉ platform_admin: một đường duy nhất, và một recompile không đổi thì không ghi gì |
| group membership (thêm, bớt, OIDC sync, xoá group) | `GroupMembershipRecompiler::lock_affected_in_tx` | `affected_services_for_group_membership` có thêm nhánh admin-policy dưới phép thử `holds_platform_admin` sẵn có |
| xoá user | `hard_delete` → `lock_affected_by_user_delete_in_tx` | nhánh role (§4.3) cho admin trực tiếp; nhánh group cho admin qua group |
| disable / enable | **mới** `egret_perm_compile::set_user_status_and_recompile(pool, user_id, status)`, gọi từ `PATCH /users/{id}` thay cho `set_status` | thêm các admin-policy service khi user giữ platform_admin trực tiếp hoặc qua group; lock; `UserRepo::set_status_in_tx`; `recompile_services_in_tx` |
| CLI `create-admin` | `egret-bin` | sau lệnh insert, một transaction chạy `recompile_services_in_tx(admin_policy_services)` |
| drift, body cũ | **boot pass** `sync_admin_policies(pool)` | spawn cạnh `recompile_role_referencing_profiles` (`main.rs:2362-2376`): lock `admin_policy_services_via` tăng dần, `recompile_services_in_tx`. Idempotent và best-effort; lỗi được log và không bao giờ chặn boot |

`premint_model_usernames` cũng mint các admin đang active khi có bất kỳ admin-policy service nào. Điều này giữ
invariant "mint tăng dần một lần, trước các pass" (`members.rs:336-358`).

Thứ tự lock không đổi: lock profile tăng dần, rồi mint lock, rồi ghi dòng. Sync chỉ ghi dòng của các service mà
caller đã lock.

### 4.3 `hard_delete` lấp các lỗ recompile

`PermGroupRecompiler::lock_affected_by_user_delete_in_tx` (chữ ký trait không đổi) thêm các phần sau, đọc trên
cùng transaction **trước** khi xoá `access_grants`:

- `affected_services_for_role_membership(name)` cho mỗi tên role khác nhau trong `access_grants` của user (mọi
  scope). Việc này kéo theo nhánh clearance, nhánh grant, nhánh engine-grant, và (với `platform_admin`) nhánh
  impersonation và admin-policy;
- `affected_services_for_workspace_members(ws)` cho mỗi grant `scope_type = 'workspace'` của user.

Hợp các tập được sort và dedup, rồi lock trong một lượt tăng dần, rồi lấy mint lock, y như các nhánh group và
engine-grant hiện có.

**Race còn sót, nói rõ.** Một role được grant đồng thời cho user, sau lần đọc này và trước khi xoá, sẽ bị
`DELETE FROM access_grants` quét đi nhưng các profile của nó không được recompile. Body của chúng khi đó ghi một
username đã retire. Ledger khiến điều đó vô hại (không ai giữ được tên đó), và lần recompile tiếp theo của các
profile ấy sẽ bỏ nó. Phần recompile có mặt để body đúng. Ledger mới là sự bảo đảm.

### 4.4 Event xoá, hướng sang Disable, UI copy

**Event.** `hard_delete(target, actor: &Principal, recompiler)` trả về `Option<DeletedUser { name, email, username:
Option<String> }>` (`RETURNING name, email, username`). Trong cùng transaction, trước commit, nó gọi
`record_change` với:

- `action::USER_DELETE = "user:delete"`, `resource::USER = "user"`, `resource_id` = id của user;
- summary `{"before": {"username", "email", "name", "provider"}, "after": null}`.

Cặp này gia nhập `FAMILIES` dưới một family mới `User` (D4) và `GOVERNANCE_ACTIONS`. Điều này phủ cả hai caller
(`DELETE /users/{id}` và việc xoá service-account).

**Ai đọc nó (D4).** `GET /governance/changes` dựng danh sách family mà người đọc được xem từ một permission cho mỗi
family (`governance_changes.rs`: `policy:read` → `Policy`, `tag:read` → `Tag`) và truyền nó cho
`governance_changes_via`, hàm này lọc `FAMILIES` theo danh sách đó. `User` gia nhập vòng lặp đó với `user:read`.
Gate đầu vào của route (`require_governance_read`: `policy:read` hoặc `tag:read`) không đổi: feed vẫn là một trang
Governance, và các dòng `user:delete` là một family thêm bên trong nó cho người đọc nào giữ thêm `user:read`. Người
đọc `policy:read` không có `user:read` không thấy dòng `user:delete` nào; principal chỉ có `user:read` vẫn nhận 403
từ feed (họ không phải người đọc Governance). Platform admin giữ cả ba. Mô tả của route và nhãn Recent changes của
Overview client (Task 8) theo đó. Route service-account giữ sink event `service_account:delete` riêng của nó và
thêm `username` vào đó.

**API copy.** Mô tả 204 của `DELETE /users/{id}` trở thành: "User hard-deleted along with sessions, group
memberships and grants. Their engine username is retired permanently: no user can be given it again, so refs and
policies naming it match nobody. To revoke access but keep the username, set `status` to `disabled` instead."

**UI: chính xác những gì thay đổi.**

1. `ControlPlaneUserDto` có thêm `username: string | null` (read-only; `null` cho tới khi được mint).
2. Dialog xoá (`UsersPage.tsx` và `UserDetail.tsx`). Tiêu đề không đổi. Mô tả trở thành, khi `username` có giá
   trị: "Deletes this user, their sessions, group memberships and role grants. Their engine username
   **{username}** is retired for good: nobody can be given it again, including this person if they come back, so
   grants and policies naming it will never apply to anyone. To stop them signing in and keep their username,
   grants and history, disable them instead." Khi `username` là null, câu thứ hai là "They have no engine
   username yet." và câu cuối giữ nguyên. Nếu user đang active, một nút **Disable instead** được truyền vào làm
   `children`: nó đóng dialog này và mở xác nhận disable sẵn có. Việc gõ `delete` và nút Delete không đổi.
3. Dialog disable (`UserDetail.tsx`, và xác nhận Disable hàng loạt trên `UsersPage.tsx`) nối thêm: "Their
   engine username stays theirs." Với platform admin, nó nối thêm: "While disabled they lose all-access on
   engines."
4. Menu dòng của `UsersPage` thêm **Disable** (user active) hoặc **Enable** (user bị disable) phía trên
   **Delete**, với cùng các guard như trang chi tiết (không phải chính mình). Hiện nay menu dòng chỉ có
   Impersonate và Delete, nên disable từng dòng cần dùng thanh bulk.
5. Governance > People có thêm tab **Reused names** (`?kind=reused`, chỉ platform admin) cho báo cáo (§4.5).
   Governance > Overview hiển thị finding mới. Recent changes hiển thị `user:delete` là "Deleted user
   {username or email}".

### 4.5 Báo cáo (D3)

**Route.** `GET /api/control-plane/perm/username-reuse`, trong `egret-perm-compile` (nó đọc `perm.*`, `policy.*`,
`users`, ledger và `audit.audit_log`). Chỉ platform admin (`Principal` giữ `platform_admin` ở platform scope), vì
nó hiện email và holder xuyên workspace. Người khác nhận 403. Read-only, `REPEATABLE READ`.

**Finding** (mỗi cái mang username, vị trí của nó (id grant, rule hay engine-grant cùng position và effect; hoặc
id policy, tên, service và `managed_by`), holder hiện tại (id, email, `created_at`, `minted_at` của ledger), mốc
thời gian bằng chứng, và một verdict):

| kind | quy tắc | verdict |
|---|---|---|
| `ref_predates_holder` | một ref `user` mà governance event sớm nhất ghi nó (`"user:<ref>"` trong các event của dòng đó), hoặc nếu không có thì `created_at` của dòng cha (và, với engine grant, `updated_at`), **cũ hơn `created_at` của holder** | `definite` khi một event hoặc `updated_at` của engine grant chứng minh điều đó; ngược lại `possible` (một PATCH có thể đã thêm ref cho holder về sau: grant và rule không có `updated_at`) |
| `policy_predates_holder` | một body policy ghi một username đang có holder mà `updated_at` cũ hơn `created_at` của holder, đọc live và từ baseline 0209 (bên dưới) | `definite` với `compiler`/`infra`; `possible` với `operator` (có thể là pre-provisioning có chủ đích) |
| `admin_policy_named_non_admin` | một body `egret-admin` ghi một người không phải platform admin đang active (hoặc không ai), đọc từ baseline 0209 và live | `definite` (dòng live phải rỗng khi D2 đã vào; có dòng live tức là sync đã fail) |
| `holderless_ref` | một ref `user` hoặc tên `users[]` viết tay không có holder, kèm dòng ledger của nó (`retired_at`, `minted_for`) và việc position của nó có fail open không (deny include, allow exclude, rule exempt exclude) | `dead` (đã retire, không khớp ai, vĩnh viễn) |

**Baseline 0209.** Boot sync (§4.2) và các recompile ghi lại đúng những body mà báo cáo cần. Vì vậy 0209 chụp
snapshot chúng một lần, trước khi bất cứ thứ gì kia chạy, vào `username_reuse_baseline(service_id, policy_id,
policy_name, managed_by, policy_updated_at, username, holder_id, holder_created_at, holder_is_active_admin,
taken_at)`. Nó chỉ giữ các dòng là bằng chứng: không có holder, `updated_at < holder.created_at`, hoặc một dòng
`egret-admin` ghi người không phải admin. Sau 0209 mọi lần ghi lại đều đúng theo cách dựng, nên không cần snapshot
nào sau đó.

**Attention signal.** Mô hình vừa vặn (E19): thêm một `Source`, kind `username_reuse`, với một pseudo-permission
mới `platform_admin` trong `held` (được insert khi principal giữ role đó; liệt kê trong `notChecked` với mọi người
khác).

- Severity `critical` khi có bất kỳ finding `definite` nào; ngược lại `warning` cho các finding `possible`.
- Không có finding khi chỉ có ref `dead`, thứ mà `unresolved_principals_fail_open` đã báo cáo rồi.
- Sample hiện username và vị trí, không bao giờ hiện email.
- `Ref::ReusedNames` liên kết tới `/governance/people?kind=reused`.

**Không có hành động tự động.** Không ai bị disable và không ref nào bị sửa (D3).

**Những gì báo cáo không thấy được.**

- **Retention:** governance event cũ hơn `retention_days` đã mất. Trước gói này, xoá một người thật không để lại
  event nào cả. Bằng chứng cũ hơn mốc đó chỉ còn các timestamp sót lại.
- **Ref đã bị gỡ:** một ref cũ về sau bị sửa mất, hoặc một grant đã bị xoá, không để lại dòng nào, chỉ có event
  trong thời hạn retention.
- **Thời điểm mint của các tên đã được giữ trước 0209:** `minted_at` là NULL, nên `users.created_at` là cận dưới.
  Một ref thêm vào giữa lúc tạo và lúc mint được đọc là `possible`.
- **Phơi nhiễm tạm thời** từ các expansion role, workspace hay clearance cũ (E6) giữa một lần xoá và lần recompile
  tiếp theo. Recompile đã ghi đè body và không để lại dấu vết, trừ khi baseline 0209 bắt được nó lúc còn cũ.
- **Văn bản tự do:** một username literal trong biểu thức row-filter hay mask hoặc trong một condition
  (`current_user = 'bob'`).
- **Quyền sở hữu phía engine:** owner của table, view và schema trong Iceberg, Trino hay StarRocks, các view
  `DEFINER` của Trino chạy dưới quyền owner, và các item `{OWNER}` của Ranger. Những thứ đó nằm trong engine hoặc
  catalog, không nằm trong egret.
- **Việc sử dụng thực tế:** liệu một quyền thừa hưởng đã từng được dùng hay chưa. Điều đó cần query log của engine
  (Trino event listener, StarRocks audit log), và egret có thể không lưu chúng. Báo cáo không liên kết log nào.
- **Body `egret-admin` đã được seed lại trước 0209** bởi một lần engine re-apply đã ghi lại chúng.

### 4.6 Engine JWT cũ của user đã bị xoá

**Quyết định: không có revocation list.** Xoá session cộng với ledger là đủ. Lý do:

- **Cửa sổ.** Một JWT mint trước khi xoá vẫn hợp lệ tối đa **900 s** sau khi mint (E17), cộng thêm poll interval
  của Ranger plugin đối với mọi thay đổi policy mà việc xoá recompile. Các query đã được nhận vẫn tiếp tục chạy.
- **Không có token mới sau khi xoá.** Session bị xoá trong cùng transaction (E5). Minter từ chối: dòng user đã mất,
  nên `ensure_username` trả `NotFound`. Token không refresh được (không có `cgen`), và nó không bao giờ rời các
  executor phía server của egret.
- **Ledger đóng nửa chuyện chiếm tên.** Về sau không ai giữ được tên đó, nên token cũ chỉ có thể dùng những gì vẫn
  ghi *chính* tên đó: các ref trực tiếp của chính người bị xoá và các mục `users[]` viết tay, trong tối đa 15 phút.
  Nó không bao giờ hành động với grant của người kế nhiệm, và không người kế nhiệm nào hành động với grant của nó.
- **Disable hiện nay có cùng hình dạng.** Điều đó được chấp nhận với Disable, và Delete không tệ hơn.

Một deny-list theo `jti` sẽ rút 15 phút xuống còn tới request tiếp theo, với giá một lần đọc store trên mỗi câu
lệnh engine. Không đáng cho một token mà chỉ egret giữ.

## 5. Nguyên tắc

- Làm cho việc dùng lại không thể biểu diễn được trong database. Guard ở caller là không đủ.
- Một định nghĩa duy nhất cho "engine admin", dùng bởi engine apply, sync và báo cáo.
- Mọi thay đổi làm đổi ai là admin đều đi qua chokepoint recompile. Không đường nào có bản sao riêng.
- Báo cáo bằng chứng. Không thay đổi gì mà user không yêu cầu thay đổi.

## 6. Thay đổi backend (liệt kê)

1. `migrations/0209_username_ledger.sql`:
   - `username_ledger`, năm trigger của nó, và `username_ledger_backfill()`;
   - `username_reuse_baseline`, điền một lần.
2. `egret-auth`:
   - docs của `ensure_username_via`;
   - `hard_delete` (actor, event, `DeletedUser`);
   - `set_status_in_tx`;
   - `roles::engine_admin_user_ids_via`.
3. `egret-audit`: `action::USER_DELETE`, `resource::USER`, `FAMILIES`, `GOVERNANCE_ACTIONS`.
4. `egret-policy`: `sync_admin_all_access_in_tx`, `admin_policy_services_via`; `seed_admin_all_access` uỷ quyền
   cho chúng.
5. `egret-perm-compile`:
   - sync trong `recompile_locked_service_in_tx`;
   - các nhánh admin-policy trong tập affected của role và group;
   - admin trong `premint_model_usernames`;
   - nhánh role và workspace trong `lock_affected_by_user_delete_in_tx`;
   - `set_user_status_and_recompile`;
   - `sync_admin_policies` (boot);
   - route báo cáo và module query;
   - sửa doc "known gap" ở `principal_refs.rs:86-99`.
6. `egret-controlplane`:
   - `delete_user` truyền actor;
   - `update_user` dùng `set_user_status_and_recompile`;
   - các route role của group dùng đường có recompile;
   - `UserDto.username`;
   - source `username_reuse`, pseudo-permission `platform_admin`, `Ref::ReusedNames`.
7. `egret-dataapi`: việc xoá SA truyền actor và thêm `username` vào event của nó.
8. `egret-engine-mgmt`: `admin_usernames` dùng `engine_admin_user_ids_via`.
9. `egret-bin`: spawn boot pass; sync của `create-admin`.
10. `egret-core`: docs của `engine_principals` (delegator được reserved qua ledger).

## 7. Wire shape

```json
{
  "generatedAt": "2026-10-23T10:00:00Z",
  "findings": [
    {
      "kind": "ref_predates_holder",
      "verdict": "definite",
      "username": "bob",
      "place": {"type": "grant", "id": "…", "position": "include", "effect": "allow"},
      "holder": {"id": "…", "email": "bob@new.example", "createdAt": "…", "mintedAt": null},
      "evidenceAt": "2026-07-01T09:00:00Z",
      "evidence": "governance event perm_grant:create named user:bob"
    }
  ],
  "baselineTakenAt": "2026-10-24T08:00:00Z"
}
```

`place.type` ∈ `grant | rule | engine_grant | policy`; place kiểu `policy` mang `{policyId, name, service,
managedBy}`. Attention kind mới là `username_reuse`, và biến thể `Ref` mới là `reused_names`.

## 8. Migration và tương thích

- **0209** đã được giữ chỗ (`origin/main` dừng ở 0207; PR đang mở #740 lấy 0208). Kiểm tra lại cả hai trên `origin/main` và trên
  các PR đang mở trước khi merge; đánh số lại cao hơn cả hai nếu cần.
- 0209 là additive. Trigger của nó có hiệu lực với mọi lần ghi về sau. Trên bản cài mới, bước 1 seed từ một
  `users` rỗng.
- Các test ghi `users.username` trực tiếp vẫn chạy, trừ khi một binary xoá một user rồi insert lại **cùng**
  username. Những test đó giờ fail với `23505`, đó là hành vi mong muốn. Plan sẽ rà soát chúng.

## 9. Rủi ro

- **Seed quá nhiều** (bước 3 trên body compiler và infra, bước 4b trên event) retire những tên chưa từng thuộc về
  một người. Cái giá là suffix `-2` cho một người tương lai có slug đó. Điều đó được chấp nhận.
- **Sync bên trong mọi recompile của một admin-policy service** thêm một query cộng một phép so sánh body cho các
  recompile đó. Nó không ghi gì khi body không đổi.
- **Việc ghi role của group giờ có recompile** (E10). Một clearing role grant cho group kích hoạt một recompile
  toàn bản cài đặt mà không đổi gì (role expansion bỏ qua group grant). Điều đó đúng và idempotent; cái giá được
  ghi nhận.

## 10. Các lỗi liên quan tìm thấy (không sửa ở đây)

- `guard_not_last_admin` đếm cả admin bị disable (E9). Disable mọi admin trừ một, rồi disable nốt người đó, vẫn
  được cho phép. Một dòng code sẽ sửa được khi `engine_admin_user_ids_via` đã có; nó được để cho follow-up vì nó
  đổi một contract 409.
- Danh tính owner của alert và report mint và chạy dưới danh nghĩa owner bị disable (`evaluator.rs:147-180`). Điều
  này chưa được kiểm chứng end to end.
- Chiếm tên role (một tên role được tạo lại thừa hưởng các ref `role`; `principal_refs.rs:94-96`).

## 11. Non-goal

Bảy follow-up khác của #692, tất cả vẫn còn mở trên `aec59807`:

1. boot repair chạy một transaction cho mỗi profile, có retry (`role_membership.rs:251-292`);
2. `policy:read` lazy trong `reference_detail` (`access/mod.rs:711-721`);
3. `old_services` lấy từ lần đọc mới trong `write_grant`/`write_rule` (`routes.rs:1177-1212`, `:1440`);
4. pre-mint dựa trên các tập được tham chiếu thay vì toàn bộ model (`members.rs:359-422`);
5. bỏ qua pre-mint khi `compile_desired` trả `None` (M2; `trigger.rs:1442-1445`, `:2183-2186`);
6. tập profile lớn lên sau mint lock sớm (`members.rs:304-312`);
7. lock timeout của `view_hold` được khớp theo text (`crates/egret-catalog-engine/src/view_hold.rs:858-864`).

Cũng không nằm ở đây:

- purge hoặc ghi lại ref khi xoá;
- từ chối xoá khi tên đang được tham chiếu;
- tự động disable các vụ nghi chiếm tên;
- deny-list theo `jti`;
- namespace riêng cho các tên `svc-`;
- liên kết một người quay lại với username cũ của họ;
- khắc phục văn bản tự do và quyền sở hữu phía engine;
- Platform docs trong `egret-data/docs` (D5: một docs PR riêng sau khi PR này merge);
- bản `docs/vi` của plan. Spec có một bản dịch tham khảo tiếng Việt ở cuối (D5).

## 12. Câu hỏi mở (đã trả lời 2026-10-23)

**Q1. Ai được đọc event `user:delete`?** **Đã trả lời (D4): một family `User` riêng, đọc bằng `user:read`.**
Phần dưới đây là câu hỏi như lúc được hỏi. `record_change` yêu cầu một family trong `FAMILIES`, và family quyết
định ai đọc. Với mặc định, `Policy`, mọi holder `policy:read` thấy username và email của user bị xoá trong
Governance > Recent changes. Phương án khác là family thứ ba, `User` (`user:read`), cần đổi bộ lọc feed
(`governance_changes_via` và Overview client). Mặc định: `Policy`, vì username là thứ người đọc policy cần để hiểu
các ref chết. Chỉ Task 2 thay đổi nếu câu trả lời là `User`.

**Q2. Platform docs và bản `docs/vi` đặt ở đâu?** **Đã trả lời (D5):** Platform docs là một docs PR riêng sau khi
code PR merge, do controller viết (plan Task 9 nằm ngoài PR này). Bản `docs/vi` của spec này là bản dịch tham khảo
tiếng Việt, viết sau khi code xong vì spec có thể còn đổi trong lúc triển khai; header của nó nói đây là bản tham
khảo, liên kết tới file này và nói bản tiếng Anh được ưu tiên.

## 13. Ghi chú triển khai (append-only)

Các sai khác so với phần trên mà code buộc phải có hoặc quá trình triển khai phát hiện, ghi lại khi chúng xảy ra.

### Task 1 (ledger)

- **Guard append-only chặt hơn §4.1.** §4.1 cho `user_id` và `retired_at` đổi tự do. Guard chỉ cho `user_id` đổi
  về NULL và `retired_at` chỉ đổi từ NULL. Set `user_id` sang user khác sẽ trao tên cho họ: trigger ghi return sớm
  khi dòng ledger đã ghi writer là holder. Một trigger `BEFORE TRUNCATE` cũng được thêm, vì
  `TRUNCATE users … CASCADE` sẽ làm rỗng ledger mà không kích hoạt row trigger nào.
- **Hai hàm SQL helper** được thêm: `username_slug(email)` (slug của bước 4a, được một test so với
  `slugify_localpart`) và `policy_body_usernames(body)` (mọi tên `users[]` trong sáu item list của một body, trừ
  macro `{…}`), dùng chung bởi bước 3 và baseline. Báo cáo (Task 6) có thể dùng lại hàm thứ hai.
- **Bước 4b cũng đọc event của engine-grant.** Summary của engine-grant ghi principal dưới dạng object
  `{"kind": "user", "ref": …}`, không phải chuỗi `"user:<name>"`, nên bước 4b đọc cả hai dạng. Nó chỉ đọc các dòng
  governance của `perm_grant`, `perm_rule` và `engine_grant` (các family mang principal).
- **`slugify_localpart_for_tests`** là một wrapper `#[doc(hidden)] pub` trong `egret_auth::users`, cho test so
  khớp slug.
- **`lower()` và ký tự non-ASCII.** SQL slug lowercase theo collation của database, có thể khác `to_lowercase`
  Unicode của Rust với vài chữ non-ASCII (ví dụ U+212A KELVIN SIGN lowercase thành `k` ASCII trong Rust). Dù cách
  nào thì mọi ký tự non-ASCII đều thành `-`, và bước 4a chỉ đọc email của service-account, vốn được dựng từ một
  slug ASCII, nên khác biệt này không thể chạm tới ledger.

### Task 2 (các nhánh của `hard_delete` và event `user:delete`)

- **Test của event nằm trong `egret-controlplane`** (`tests/governance_changes.rs`), không nằm trong
  `egret-perm-compile/tests/user_delete_recompile.rs`: chúng gọi `DELETE /users/{id}` qua router control-plane, và
  `egret-perm-compile` không thể phụ thuộc `egret-controlplane`. `username` của service-account được assert trong
  `egret-dataapi/tests/audit.rs`.
- **Hiện không có đường 23503 thật** (mọi FK tới `users` đều có rule `ON DELETE`), nên test "xoá bị từ chối không
  để lại event" tự tạo một bảng riêng với một FK trần tới `users` trên test DB per-process.
- **`DeletedUser` cũng mang `provider`**, thứ mà summary của event cần (§4.4); `RETURNING` đọc
  `username::text`.
- **Event được ghi sau recompile, ngay trước commit.** Mọi lỗi trước đó (23503 của việc xoá dòng, một recompile bị
  từ chối) không để lại event; lỗi khi insert event sẽ làm việc xoá bị từ chối.
- **Feed (D4).** `governance_changes.rs` thêm `("user:read", Family::User)` vào vòng lặp family; gate đầu vào
  không đổi. Câu hiển thị là "{actor} deleted user {username, else email, else id}" không có link target.

### Task 3 (một tập engine-admin, writer trong transaction)

- **`seed_admin_all_access` vẫn dựng body từ tên và type mà caller truyền vào**, không phải của dòng: một test
  engine-mgmt sẵn có adopt một Trino profile qua đường StarRocks, và các test policy truyền tên service trong body
  khác với dòng. `sync_admin_all_access_in_tx` trong transaction đọc cả hai từ dòng. Guard infra-name dùng type
  của dòng ở cả hai, như `upsert_infra_policy` đã làm.
- **Tập rỗng:** các body đang có được ghi lại thành `users: []`; khi có user, các hierarchy còn thiếu được tạo
  ("create on apply" của apply). `seed_admin_all_access` trả về số policy `egret-admin` mà service có sau đó.
- **`is_admin_all_access_policy_name`** được thêm vào `egret_policy_api::infra_names`, để predicate chỉ có một
  cách viết.
- **Không có test `engine_apply_seeds_the_same_set` riêng.** `admin_usernames` là private của một đường apply cần
  provisioner; giờ nó gọi `engine_admin_user_ids_via` và sort, và các test route engine-mgmt sẵn có seed admin vẫn
  xanh.
- **Ranger evaluator chỉ mô hình hoá plugin 2.8.0 cho Trino** (2.5.0 bị từ chối), nên các assertion mức engine
  của Trino khai báo 2.8.0.

### Task 4 (`egret-admin` qua chokepoint recompile)

- **Sync kiểm tra membership bằng `admin_policy_services_via`** (một query) ở đầu mỗi
  `recompile_locked_service_in_tx`, và username lấy từ `members::engine_admin_usernames_via`, hàm này mint tên còn
  thiếu dưới mint lock (thường thì lượt pre-mint đã làm trước).
- **`replace_platform_roles` không có test riêng ở đây.** Nó tính tập affected bằng cùng
  `affected_services_for_role_membership`, mà nhánh admin-policy của nó được `unassigning_platform_admin_drops_them_from_
  egret_admin` và `assigning_platform_admin_adds_them` kiểm; `egret-perm-compile` không thể phụ thuộc
  `egret-workspace`. Suite `egret-workspace` vẫn xanh.
- **Một test sẵn có bị đổi:** test "the target becomes an engine admin" của `engine_grants_compile.rs` seed một body
  `egret-admin` cho một non-admin rồi recompile. Recompile giờ ghi lại body đó thành các admin thật, nên test biến
  target thành platform admin luôn.
- **Test group seed thẳng arrangement** (`sync_admin_all_access_in_tx` với tên của member), để mỗi arrangement đứng
  vững trên code trước fix và cái đỏ chính là thay đổi đang được test.

### Task 5 (status, route group-role, CLI, boot pass)

- **Test route** nằm trong `egret-controlplane/tests/admin_policy_routes.rs` và assert body đọc ngay sau response
  (`PATCH /users/{id}` disable/enable; `POST`/`DELETE /groups/{id}/roles`). Các quyết định mức engine cho cùng các
  hàm (`set_user_status_and_recompile`, group unassign, boot pass) nằm trong
  `egret-perm-compile/tests/admin_policy_sync.rs`.
- **Boot pass là một transaction** trên mọi admin-policy service (`recompile_services_in_tx`), spawn cạnh
  `recompile_role_referencing_profiles` và có log, không bao giờ chặn boot.
- **Không có test `create_admin_cli_syncs`.** `init` là private của binary `egret` và cần một `Config` đầy đủ; nó
  gọi cùng `sync_admin_policies` mà test boot pass đã phủ, và lỗi ở đó chỉ log (lần boot tiếp theo sẽ sửa).

### Task 6 (báo cáo và attention signal)

- **Wire shape, như đã dựng** (`UsernameReuseReport`): `{generatedAt, findings, baselineTakenAt}`. Một finding là
  `{kind, verdict, username, place, holder, retired, evidenceAt, evidence}`. `place` là một object phẳng,
  `{type, id?, position?, effect?, policyId?, name?, service?, managedBy?}` (field vắng thì bị lược), đơn giản hơn
  cho client sinh tự động so với một tagged union. `holder` là `{id, email, createdAt, mintedAt}` hoặc `null`.
  `retired` chỉ có với `holderless_ref`: `{retiredAt, mintedFor, failsOpen}`. `baselineTakenAt` là `null`
  khi 0208 không giữ dòng baseline nào.
- **`holderless_ref` từ policy** chỉ phủ body viết tay (`operator`), live (body compiled và infra được compiler và
  sync ghi lại). `failsOpen` của nó là true khi tên nằm trong `denyPolicyItems` hoặc `allowExceptions`.
- **Dòng baseline không có holder không được báo cáo** (các quy tắc live phủ body hiện tại); dòng baseline chỉ cấp
  cho `policy_predates_holder` và `admin_policy_named_non_admin`.
- **Gate là role `platform_admin` trên principal** (`Principal::platform_roles`), cùng pseudo-permission mà
  `attention()` insert vào `held`.
- **Attention:** kind `username_reuse`, `Requires::All(["platform_admin"])`, sample label = username, detail
  = vị trí bằng lời, sample ref = grant, rule hoặc profile (không có với engine grant); finding ref
  `{"type": "reused_names"}`.
- **Test:** `egret-perm-compile/tests/username_reuse_report.rs` (từng quy tắc, gate 403/200) và
  `egret-controlplane/tests/governance_attention.rs` (finding cho admin, `notChecked` cho người đọc
  `policy:read`, không có finding cho ref chết).

### Theo sau review (review độc lập, 2026-10-23)

- **M1: tên mint sau 0208 không bao giờ là finding.** Khi dòng ledger của holder là một mint (`minted_at` có giá
  trị), báo cáo bỏ qua quy tắc ref và policy. So với `minted_at` thay cho `created_at` thì không sửa được: một grant
  từ 2025 cũ hơn mọi mint sau 0208. Điều đúng là 0208 đã seed mọi tên mà một ref, body hoặc event còn trong
  retention biết, nên một tên được mint thì chưa từng được ghi ở đâu trước khi holder nhận nó. Test báo cáo giờ dựng
  holder trước 0208 bằng một dòng ledger backfill (`pre_0208_holder`): `INSERT` thẳng một username là một mint.
- **M2: dòng `egret-admin` chỉ nói điều đã biết, với verdict mới `stale`.** Với body `egret-admin` ghi một người
  không phải platform admin đang active (holder đọc live, nên holder đã bị xoá thì không còn là holder):
  - không ghi ai: không có finding (tên đã retire và không khớp ai);
  - holder giữ grant `platform_admin` ở bất kỳ trạng thái nào (admin bị disable), hoặc được mint sau 0208: không
    phải reuse. Dòng baseline thì không có finding (sync đã sửa body đó). Dòng live là `stale`: "vẫn cho quyền ALL
    trên engine cho một người không còn là platform admin đang active";
  - body được ghi lần cuối trước khi holder tồn tại: `definite`;
  - còn lại là `possible`. Ở đây không phân biệt được admin bị hạ quyền với takeover: username được mint lần dùng
    đầu tiên, sau `created_at`, nên `created_at` cũ hơn body không chứng minh họ giữ tên khi body được ghi.

  Với các quy tắc này không còn dòng baseline nào là `critical` sai vĩnh viễn, nên không thêm cơ chế acknowledge.
  Overview tính `stale` là `warning`.
- **L8: câu trên Overview nói riêng từng verdict**, ví dụ "2 references or policies were written for a username
  before the person who holds it now existed. 1 more may have been. 1 egret-admin policy still gives engine ALL
  access to someone who is no longer an active platform admin." Empty state của Reused names là "No reused engine
  usernames found." và nói rằng lịch sử quá hạn retention của audit thì không kiểm được.
- **M3: các event được đọc một lần.** `REFS` lấy mọi tên từ các mutation event `perm_grant`, `perm_rule` và
  `engine_grant` trong một CTE (`DISTINCT ON` lấy cái sớm nhất theo vị trí và tên) rồi join, thay cho một lần quét
  `LATERAL` cho mỗi ref. Không thêm index. Đo bằng `EXPLAIN ANALYZE` trên một bản sao của test template đã migrate,
  với 2.000 user ref và 40.000 dòng audit (20.000 dòng là mutation `perm_grant` ghi các ref đó): trước 43,6 s (51,0 s
  khi tắt JIT), sau 3,5 s (0,39 s khi tắt JIT; phần lớn 3,5 s là JIT compile ở máy local).
- **M4, M5, I1: đổi status chỉ chạy sync `egret-admin`, không bao giờ compile.** `set_user_status_and_recompile`
  luôn khoá các admin-policy service (bất kể user là ai), rồi mint lock, rồi ghi status, rồi sync từng admin-policy
  service. Áp dụng cho cả hai chiều, trong một transaction.
  - Vì sao không "commit status và thu hồi session trước, rồi recompile": như vậy để lại một khoảng thời gian (và
    sau một recompile thất bại thì không giới hạn) mà admin bị disable vẫn giữ quyền ALL trên engine.
  - Vì sao không compile đầy đủ: status không đổi gì khác trong model đã compile. User bị disable được ghi tên như
    user active, và kiểm tra impersonation đọc grant `platform_admin` (`is_platform_admin`), không đọc status.
    Compile vì thế chỉ thêm một sự phụ thuộc: một profile compile lỗi là chặn luôn kill switch.
  - Các lỗi còn lại có thể xảy ra là lỗi database, hoặc mint thất bại username còn thiếu của một admin khác.
    Session vẫn bị thu hồi sau commit.
  - Khoá trước khi đọc đóng race với một grant `platform_admin` chạy đồng thời (M4). Lấy mint lock trước khi ghi
    `users` theo đúng thứ tự của compile (I1).
  - Test: `disabling_an_admin_does_not_depend_on_a_profile_compiling` làm hỏng compile của một profile bằng một
    trigger chỉ dùng cho test trên `perm.compile_runs`. `a_disable_racing_an_admin_grant_leaves_them_out_of_egret_admin`
    giữ dòng của user để lệnh disable chờ ở bước ghi trong khi một grant chạy.
- **L1: boot pass chỉ là sync, một transaction cho mỗi service.** Service nào lỗi thì được log và bỏ qua. Một profile
  không compile được vẫn được sửa tập `egret-admin`.
- **L2: các dòng `egret-admin` mà def hiện tại không dựng cũng được ghi lại.** Điều này áp dụng cho hierarchy của def
  cũ, hoặc body seed từ def của loại engine kia. Mỗi `policyItems[].users` được đặt thành tập hiện tại, resource giữ
  nguyên. Các dòng này được tính vào `present`.
- **L3 không tái hiện được; không đổi gì.** Trên PostgreSQL 16.15, `strict $.** ? (…).ref` trên một object không có
  `ref` không trả về gì: accessor đi qua `.**` không báo lỗi thiếu key. `strict $ ? (…).ref` thì có báo.
  `the_backfill_skips_an_event_object_with_no_ref` ghim hành vi này, và nó pass cả trước lẫn sau. CTE event của báo
  cáo dùng cùng đường dẫn với bước 4b.
- **L4: mint bắt đầu sau hậu tố số lớn nhất mà ledger giữ cho base.** Nó đọc hậu tố từ 1 đến 9 chữ số và thử chính
  base trước. Insert không race vẫn giữ nguyên: lần đọc ledger chỉ chọn chỗ bắt đầu thử. Vẫn tối đa 999 lần thử.
- **L5: tên chết trong body viết tay nói cách sửa.** Áp dụng khi dòng ledger của nó là backfill của 0208 không ghi
  holder: "… if it was written for someone who had not signed in yet, rewrite it to the name they were given". Tên
  mà ledger không giữ (pre-provision sau 0208) không phải finding: nó còn trống, và người tiếp theo có email slug ra
  tên đó sẽ nhận nó.
- **Release note (cho PR body):** "Upgrading retires every username that a policy, grant, rule or retained
  governance event names but no user holds. A hand-written policy that named someone who had not signed in yet
  will not apply to them: they are given another name (for example `alice-2`). Governance > People > Reused names
  lists such names as dead. Rewrite those policies to the name the person was given."
- **L6:** mục Disable trên menu dòng của chính bạn vẫn được liệt kê, `aria-disabled`, với "You cannot disable your
  own account" là chữ hiển thị trong mục. Đây là mẫu lý do trong menu của `KvTable` và `FileTree`. Mục Delete giữ
  `title` cũ, không đổi ở đây.
- **L7: "Disable instead" chỉ hiện khi người xem có quyền disable** (`isAdmin`, như Disable trên dòng). Với một user
  mà danh sách cho thấy là platform admin active cuối cùng, nút là `aria-disabled` kèm lý do
  (`lastActiveAdminReason`). Danh sách chỉ có role trực tiếp, nên admin chỉ qua group không được đếm, và 409 của
  server vẫn là câu trả lời cuối cùng.
- **I2: runbook, ledger là dữ liệu.** Restore schema và dữ liệu cùng nhau (một `pg_restore` đầy đủ vào database
  trống).
  - Restore chỉ dữ liệu vào một database vừa migrate sẽ xung đột với hai dòng `reserved` mà 0208 đã insert
    (`egret-lineage`, `egret_delegator`). Lệnh COPY của ledger khi đó lỗi và bảng để trống.
  - Không có `--disable-triggers`, các trigger của `users` cũng chạy trên mọi dòng được restore và mint lại các tên.
  - Nếu không tránh được restore chỉ dữ liệu: chạy bằng superuser với `--disable-triggers`. Trước đó, xoá các dòng
    reserved trong một session có `SET session_replication_role = replica` (nếu không, guard append-only sẽ từ chối
    lệnh xoá), để bản sao của chúng trong dump được restore.

### Đánh số lại thành 0209 (live check, 2026-10-24)

- **Migration giờ là `0209_username_ledger.sql`.** `origin/main` dừng ở `0207`, và PR đang mở #740
  (`fix/u4-sqllab-ux`) thêm `0208_query_history_clear.sql`. Quy tắc là đánh số cao hơn main và mọi PR đang mở, nên
  file chuyển sang `0209`. Nội dung không đổi. Comment trong code, hậu tố evidence của báo cáo (giờ là
  " (at migration 0209)"), mô tả OpenAPI, helper test (`pre_0209_holder`) và §2 đến §8 ở trên giờ ghi 0209. Các ghi
  chú trước đó trong mục này giữ "0208": chúng ghi lại điều đúng lúc được viết, và mọi "0208" trong đó là migration
  này.
- **Một finding cho mỗi tên ở mỗi chỗ.** Một policy ghi một username bị dùng lại, vừa live vừa có trong baseline,
  từng bị báo hai lần: một lần từ body live và một lần từ dòng baseline, chỉ khác hậu tố evidence. Live check đếm 10
  finding cho 6 chỗ, và số đếm của Overview bị gấp đôi. Giờ báo cáo lấy khoá finding là (username, loại chỗ, id chỗ
  hoặc policy id, position) và giữ một finding cho mỗi khoá. Kind không nằm trong khoá: một chỗ chỉ cho một kind.
  Tên policy, service và `managedBy` cũng không nằm trong khoá, vì baseline giữ chúng như lúc chụp. Khi cả hai dòng
  cùng báo một chỗ, verdict mạnh hơn thắng (definite, rồi possible, rồi stale, rồi dead), và body live thắng khi
  hoà. `definite` từ baseline thắng `possible` từ live: một body đã được chứng minh là viết trước khi holder tồn tại
  thì vẫn là bằng chứng sau một lần ghi lại về sau (quy tắc của M2 cho body được ghi lại sau khi holder vào). Test:
  `a_policy_both_live_and_in_the_baseline_is_one_finding`, `the_stronger_verdict_wins_when_live_and_baseline_disagree`.
