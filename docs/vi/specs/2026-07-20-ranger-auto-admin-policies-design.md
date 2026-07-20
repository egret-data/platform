# Ranger tự tạo policy admin khi provision (Security e2e — Sub-project A) — Thiết kế

> Thuộc chuỗi "full security flow". Sub-project **A** trong ba phần:
> **A** = Ranger auto-permission cho platform admin khi provision (tài liệu này) →
> **B** = enforce per-user permission xuyên SQL Lab → dataset → chart → dashboard
> → notebook (sau) → **C** = notebook write isolation theo namespace mỗi
> workspace (sau).

## 1. Mục tiêu

Khi provision engine (Trino **hoặc** StarRocks) với **Ranger bật** và **delegation
per-user** (impersonate hoặc JWT), egret **tự động** tạo một policy Ranger
**all-access** cấp toàn quyền dữ liệu cho từng **user platform-admin**, khóa theo
**engine username** của user đó. Hiệu ứng giống cách Apache Ranger seed default
admin policy khi tạo service: engine vừa provision (được Ranger governed) là admin
query được ngay, **không cần tạo policy thủ công**.

Đây là nền tảng cho sub-project B và C — các test đó cần một user đã có quyền dữ
liệu thật.

## 2. Hiện trạng (đang có)

- **Provision chỉ auto-seed policy "hạ tầng".** Ở
  `crates/egret-engine-mgmt/src/engines.rs` (~622–628), khi
  `config.auth.is_some() && engine.is_impersonate_delegation()`, egret inject
  delegator user và gọi `seed_delegator_policies(st, service_name)`
  (~1700–1731) — tạo đúng 2 policy: `egret-delegator-impersonate` và
  `egret-query-timeout`. **Không policy nào cấp quyền select/table thật.**
- **Seeder all-access có sẵn nhưng thủ công + theo GROUP.**
  `PolicyStore::seed_default_trino_policies(service_id, service_name,
  admin_principal)` (`crates/egret-policy/src/default_policies.rs:94`) tạo 1 policy
  cho mỗi leaf của cây resource **Trino** service-def, cấp
  `policyItems:[{ "groups":[admin_principal], "users":[], "accesses": <all> }]`.
  Nơi gọi **duy nhất** là endpoint thủ công `POST
  /policies/services/{service}/seed-defaults` (`admin/mod.rs:655–664`), gate
  `policy:seed`, hardcode principal = group `"admin"`. **Không** gọi khi provision.
- **Chưa có bản StarRocks.** Có StarRocks service-def
  (`service_def.rs`, `starrocks_service_def()`, resource-only — không tag policy)
  nhưng **chưa** có `seed_default_starrocks_policies`.
- **Seam username per-user.** `ensure_username(user_id)` map principal → engine
  username; dùng chung bởi SQL Lab (`egret-query/.../routes.rs:129,149`), BI
  (`egret-bi/src/routes.rs:1821`), notebook (`egret-notebook/src/runtime.rs:34`).
- **Định danh platform-admin.** `PrincipalContext::is_platform_admin()`
  (`egret-core/src/context.rs:31`) = true khi role chứa `"platform_admin"`. **Chưa**
  có query "liệt kê mọi user có `platform_admin`" — cần thêm.

## 3. Yêu cầu (chốt từ brainstorm)

| Quyết định | Chọn |
| --- | --- |
| Cấp cho ai | **Username engine của user admin cụ thể** (không phải group) |
| User nào | **Mọi user có role `platform_admin`** |
| Phạm vi | **All-access** trên mọi resource hierarchy (kiểu default-admin của Ranger) |
| Engine | **Trino và StarRocks** |
| Kích hoạt | **Tự động khi provision** nếu Ranger bật + delegation impersonate/JWT |

## 4. Kiến trúc

Bốn thành phần. Phần build policy nằm ở `egret-policy` (không phụ thuộc identity);
caller (đường provision) resolve *ai là admin* và truyền username xuống.

### 4.1 Thành phần 1 — tổng quát hóa seeder all-access (users + cả 2 engine)

**File:** `egret-policy/src/default_policies.rs`, `.../service_def.rs` (tái dùng),
`.../lib.rs` (export).

- Tách phần duyệt cây resource (`parse_resources`, `is_hierarchy_leaf`, chuỗi
  root→leaf, `all_access_types`) thành core private nhận **service-def `Value`** và
  **grantee** (`users: &[String]`, `groups: &[String]`), trả về các policy đã dựng.
  Core này engine-agnostic — chạy cho mọi Ranger service-def.
- Viết lại `seed_default_trino_policies` để gọi core với Trino service-def +
  `groups:[admin_principal]` (giữ nguyên hành vi — endpoint thủ công vẫn chạy).
- Thêm `seed_admin_all_access(service_id, service_name, service_def, users:
  &[String]) -> Result<usize>` gọi core với service-def cho trước + `users:[…],
  groups:[]`. Tên policy **idempotent, ổn định** theo leaf (vd
  `egret-admin-allaccess-<resource-path>`) và gọi `PolicyStore::create_policy` kiểu
  **create-or-update theo (service, name)** để re-provision / rolling update không
  nhân đôi, không lỗi.
- Caller truyền Trino service-def hoặc StarRocks service-def
  (`starrocks_service_def()`) → **parity StarRocks chỉ là đổi service-def**, không
  cần logic seeder riêng. (StarRocks resource-only, service-def đã mã hóa sẵn;
  không phát tag policy.)

### 4.2 Thành phần 2 — liệt kê grantee platform-admin

**File:** store identity/RBAC (`egret-core`/`egret-identity`/nơi lưu grant role
platform) + handler provision trong `egret-engine-mgmt`.

- Thêm query `platform_admin_user_ids() -> Vec<UserId>` trả mọi user có role
  `platform_admin`.
- Trong đường provision, map từng id qua `ensure_username(user_id)` (đúng seam các
  surface query dùng) để dựng `admin_usernames: Vec<String>`. Bỏ id không resolve
  được username. De-dup.

### 4.3 Thành phần 3 — nối vào đường provision

**File:** `egret-engine-mgmt/src/engines.rs`.

- Ngay sau lời gọi `seed_delegator_policies(...)`, khi có Ranger service và
  delegation bật, gọi thêm `seed_admin_all_access(...)` với service-def của engine
  (Trino hoặc StarRocks theo `engine.kind`) + `admin_usernames`.
- **Gate:** có Ranger service **và** delegation là impersonate hoặc JWT (per-user
  identity tới engine). Khi delegation `none` hoặc deploy non-Ranger → **bỏ qua**.
  (Gate delegator hiện dùng `is_impersonate_delegation()`; khi làm plan xác nhận
  JWT-mode có vào nhánh này không, nới gate nếu cần để cả impersonate lẫn JWT đều
  auto-seed.)
- Handler provision cần truy cập store identity để resolve admin (Thành phần 2).
  Nếu state `egret-engine-mgmt` chưa có, thread một `AdminUsernameResolver` nhỏ
  (boxed async fn / trait) vào state để lớp policy vẫn không phụ thuộc identity.
- **Idempotent & rolling update:** bật JWT sẽ re-provision coordinator; seeder phải
  chạy lại an toàn (create-or-update theo tên), giống `seed_delegator_policies`.

### 4.4 Thành phần 4 — e2e

**File:** `frontend/tests/e2e/ranger-auto-admin-kind.spec.ts` (mới), tái dùng
`helpers.ts`; chạy trên deploy Ranger-on (`deploy-kind-governance`) với
`EGRET_E2E_KIND=1`. Thêm `just` target (vd `e2e-kind-ranger-auto`) hoặc gộp vào
`e2e-kind-governance`.

Test chứng minh auto-policy — điểm khác cố ý so với `governance-kind.spec.ts` là
**không seed policy access thủ công**:

1. Provision Trino Ranger-bound → bật JWT → gán workspace (tái dùng
   `provisionStorageAndWarehouse`/`provisionTrinoEngine`/`enableEngineJwt`/
   `assignEngineToWorkspace`).
2. Seed 1 bảng Iceberg governed (vài dòng) qua engine.
3. Là platform admin, chạy `SELECT * FROM <table>` ở SQL Lab (API `runQuery`).
   **Assert thành công + trả đủ dòng** — quyền đến *hoàn toàn* từ policy admin
   auto-tạo, không `seed-defaults` thủ công, không policy viết tay.
4. Assert policy admin all-access tồn tại qua API policy (GET policies của service
   → có policy mà `policyItems[].users` chứa username admin).
5. Bản StarRocks: cùng luồng với engine StarRocks đã provision — chứng minh seeder
   theo service-def chạy cho cả hai.

Cleanup (xóa engine) chạy trong `finally` với `request` sống, theo convention
kind-e2e.

## 5. Luồng dữ liệu

```
provision engine (Ranger on, delegation=impersonate|jwt)
  └─ engines.rs provision path
       ├─ ensure_delegator_secret + inject_delegator_user
       ├─ seed_delegator_policies(service)                 // plumbing sẵn có
       ├─ admins = platform_admin_user_ids()               // Thành phần 2
       │    .map(ensure_username).dedup()
       └─ seed_admin_all_access(service, <engine service-def>, admins)  // Thành phần 1
            └─ mỗi leaf: create-or-update policy Ranger
               { users: admins, groups: [], accesses: <all> }

sau này — admin chạy query (SQL Lab / BI / notebook):
  OIDC principal → ensure_username → per-user JWT / X-Trino-User
    → Trino/StarRocks → Ranger plugin check policy → allowed
```

## 6. Edge case & xử lý lỗi

- **Nhiều platform admin** → một bộ policy với mọi username trong `users:`.
- **Không resolve được admin nào** (list rỗng) → không seed (log warning);
  provision vẫn thành công.
- **Delegation = none / deploy non-Ranger** → bỏ hẳn auto-seed.
- **Re-provision / bật JWT rolling update** → create-or-update idempotent theo tên;
  không nhân đôi, không lỗi.
- **Ranger không tới được lúc seed** → báo warning provision rõ ràng nhưng không
  fail cả provision (giống `seed_delegator_policies`; xác nhận khi plan). Admin có
  thể re-provision hoặc dùng `seed-defaults` thủ công như fallback.
- **StarRocks resource-only** → service-def chỉ cho resource hierarchy; không tag.

## 7. Chiến lược test

- **Rust unit** — core tổng quát dựng đúng JSON policy cho service-def +
  `users`/`groups` (assert `policyItems`, `accesses`, chuỗi resource) cho cả Trino
  và StarRocks service-def; tên idempotent.
- **Rust unit** — `platform_admin_user_ids()` trả đúng các user có `platform_admin`.
- **e2e (kind, Ranger-on)** — Thành phần 4 (Trino + StarRocks): provision → admin
  query thành công từ auto-policy.
- Chạy đủ CI local trước merge (fmt/clippy/workspace test; FE lint/typecheck/test/
  build) theo convention.

## 8. Ngoài phạm vi (sub-project này)

- **B** — assert enforcement per-user xuyên SQL Lab → dataset → chart → dashboard →
  notebook (A chỉ chứng minh đường admin all-access ở SQL Lab).
- **C** — notebook write isolation theo namespace mỗi workspace.
- UI quản lý policy cho user thường, workflow grant per-user ngoài admin bootstrap,
  và mọi thay đổi endpoint `seed-defaults` thủ công (giữ nguyên).

## 9. Câu hỏi mở (giải quyết khi làm plan)

1. JWT-mode delegation có vào cùng nhánh `engines.rs` như impersonate không, hay
   gate `is_impersonate_delegation()` quá hẹp? Nới nếu cần.
2. Vị trí + chữ ký chính xác của `platform_admin_user_ids()` (store nào giữ
   role-assignment) và `egret-engine-mgmt` state đã có identity access chưa hay cần
   thread resolver vào.
3. `PolicyStore::create_policy` đã upsert theo (service, name) chưa hay cần thêm
   đường update để idempotent.
