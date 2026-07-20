# Notebook write isolation (Security e2e — Sub-project C) — Thiết kế

> Thuộc chuỗi "full security flow". Sub-project **C** trong ba phần:
> A (Ranger auto-admin policies — **đã merge** #293) → B (per-user enforcement —
> **đã merge** #294) → **C = notebook write isolation theo namespace mỗi workspace
> (tài liệu này).**

## 1. Mục tiêu

Mỗi workspace có một **namespace Iceberg riêng** (trong "notebook warehouse" do
admin cấu hình). Kernel notebook nhận **catalog token scoped vào namespace đó**,
Daft **đọc + ghi TRỰC TIẾP** (plane scratch nhanh, confined) — token chỉ cho phép
namespace đó, nên ghi ra ngoài bị catalog guard từ chối. **Dữ liệu governed/chia sẻ
đọc qua Trino/StarRocks** (đã sẵn vậy). Đây là mô hình "2-plane": scratch Iceberg
trực tiếp + đọc governed qua engine.

## 2. Hiện trạng (đã khảo sát)

- **Đọc đã governed.** `trino(sql)`/`starrocks(sql)` chạy dưới engine JWT per-user
  (`engines.py:106,186`); kernel KHÔNG đọc Iceberg trực tiếp. Yêu cầu "đọc qua
  engine" đã đạt.
- **Ghi đi QUA engine và KHÔNG confined.** `write_table(df, "catalog.schema.table")`
  chạy `df.write_sql(...)` qua Trino/StarRocks (`engines.py:210-258`), nhận BẤT KỲ
  target, dựa hoàn toàn vào Ranger của engine. Chưa có ghi Iceberg trực tiếp, chưa
  có namespace riêng, chưa có catalog token cho kernel.
- **Máy móc authz đã có sẵn.** `EgretCatalogGuard` (`crates/egret-bin/src/main.rs:275-389`)
  enforce warehouse-prefix isolation (prefix URL phải khớp claim `warehouse` →
  `lakekeeper_warehouse_id`, main.rs:343) + grant read/write theo namespace
  (`catalog_grant_covers`, main.rs:407-440: grant `securable_type:"namespace"` khớp
  namespace CHÍNH XÁC, phủ mọi table trong đó; write cần `privilege ∈ {write,
  manage}`, read chấp nhận mọi quyền). Machine-IdP mint JWT `aud:"catalog"`; kernel
  image đã có sẵn `pyiceberg[s3fs]` + `daft` (`kernel-image/Dockerfile:14`). Nên C
  chủ yếu là WIRING chúng vào notebook.

## 3. Yêu cầu (chốt từ brainstorm)

| Quyết định | Chọn |
| --- | --- |
| Mô hình ghi | **Iceberg trực tiếp, confined** vào namespace mỗi workspace (không qua engine) |
| Truy cập ns riêng | **Đọc + ghi trực tiếp** (rw scratch) qua catalog token scoped-namespace |
| Nằm ở đâu | **Notebook warehouse do admin cấu hình** (1 workspace setting) + namespace reserved |
| Granularity | **Per-workspace** (chia sẻ giữa thành viên ws) |
| Đọc governed | không đổi — qua `trino()`/`starrocks()` |

## 4. Kiến trúc

Sáu thành phần. Backend wire catalog token confined vào runtime manifest; kernel
thêm helper ghi Iceberg trực tiếp.

### 4.1 Workspace setting — notebook warehouse + namespace

**File:** `crates/egret-notebook/src/{model,store,routes}.rs` (+ migration).

- Setting mỗi workspace `{ warehouse_id: Uuid, namespace: Vec<String> }` (namespace
  mặc định `["notebook"]`). Lưu bảng mới
  `analytics.notebook_warehouse_settings(workspace_id PK, warehouse_id uuid,
  namespace text[])`, theo precedent `analytics.notebook_engine_access`
  (`store.rs:543-551`).
- Expose qua API notebook-settings sẵn có: mở rộng `NotebookSettings`
  (`model.rs:104`) + `UpdateNotebookSettings` (`model.rs:110`) thêm warehouse id +
  namespace, và `GET/PUT /api/ws/{workspace}/notebook-settings` (`routes.rs:93`,
  PUT gate `workspace:manage_members`). Validate warehouse thuộc workspace (thêm
  filter `list_warehouses_by_workspace`, chưa có).

### 4.2 Catalog role + grant scoped-namespace rw mỗi workspace

**File:** `crates/egret-notebook` (provisioning) gọi store methods
`egret-catalog-engine` in-process (HTTP handler gate platform-admin nên gọi store
trực tiếp).

- Khi set notebook warehouse (idempotent), đảm bảo có `catalog_role`:
  `create_role(warehouse_id, "notebook", consumer_type="notebook", …)`
  (`embedded.rs:235`) + `add_grant(role_id, GrantInput{ securable_type:"namespace",
  namespace:Some(namespace), table_name:None, privilege:"write" })`
  (`embedded.rs:286`). Một grant `write` namespace = full rw đúng namespace đó (§2).
- Idempotent (`add_grant` dedupe; `create_role` create-or-get).

### 4.3 Mint catalog-JWT in-process cho kernel

**File:** `crates/egret-notebook/src/runtime.rs` (+ helper mint nhỏ dùng
`egret-machine-idp`).

- Thêm helper mint JWT `aud:"catalog"` giống `machine-idp/src/lib.rs:189-198`:
  `sign_claims(&keys.active().await?, &json!({ "iss":issuer, "sub":<notebook>,
  "aud":"catalog", "warehouse":<lakekeeper_warehouse_id>, "roles":[<notebook role>],
  "exp":… }))` (`token.rs:9`). Không phụ thuộc Keycloak/refresh-token (khác engine
  JWT). State cần truy cập `PgSigningKeyStore` (catalog guard đã dùng in-process).
- Scope: per-workspace (role/grant per-workspace); mint mỗi kernel/user×workspace,
  TTL ngắn (theo engine JWT ~1h + endpoint refresh — xem §6).

### 4.4 Mở rộng runtime manifest + kernel env

**File:** `crates/egret-analytics-api/src/dto.rs` (`NotebookRuntime`),
`crates/egret-notebook/src/runtime.rs`, `kernel-image/egret_kernel_helpers/runtime.py`.

- Thêm vào `NotebookRuntime` (`dto.rs:41-55`): `catalog_jwt`,
  `catalog_jwt_expires_at`, `catalog_uri` (base REST = `{catalog_base_url}/catalog`,
  tức mount `/catalog/v1` — `egret-server/src/lib.rs:293`), `catalog_warehouse` (id
  lakekeeper warehouse guard so prefix, main.rs:343), `namespace: Vec<String>`.
- Populate trong `build_notebook_runtime` (`runtime.rs:26-66`) — chỉ khi workspace
  có setting notebook-warehouse; nếu không, `catalog_jwt=None` (helper ghi báo lỗi
  rõ "chưa cấu hình notebook warehouse").
- `manifest_to_env` (`env.rs:18-27`) đã serialize cả manifest vào
  `EGRET_NOTEBOOK_RUNTIME`; field mới đi kèm. Mở rộng `runtime.py:14-26` để đọc (hiện
  chỉ đọc `username` + `engines`).

### 4.5 Helper kernel — `write_iceberg` / `read_iceberg`

**File:** `kernel-image/egret_kernel_helpers/` (module mới, export + inject).

- `write_iceberg(df, name, mode="append")` và `read_iceberg(name)` chỉ thao tác trên
  namespace reserved của workspace — target luôn là `{namespace}.{name}` (caller chỉ
  đưa tên table `name`, không bao giờ catalog/schema), nên helper không thể trỏ ra
  ngoài ns riêng.
- Dựng `pyiceberg.catalog.rest.RestCatalog("egret", uri=<catalog_uri>,
  token=<catalog_jwt>, warehouse=<catalog_warehouse>)`;
  `create_namespace_if_not_exists(tuple(namespace))`; `create_table_if_not_exists` /
  `load_table`; `df.write_iceberg(tbl, mode=…)` và `daft.read_iceberg(tbl)`. `token=`
  set Bearer guard validate; guard từ chối ghi ngoài namespace được grant.
- Đọc governed vẫn qua `trino()`/`starrocks()`; `write_table` (engine INSERT) giữ
  cho ghi governed. `write_iceberg` là plane scratch riêng MỚI.

### 4.6 e2e test

**File:** `frontend/tests/e2e/notebook-write-isolation-kind.spec.ts` (mới) + just
recipe. Opt-in `EGRET_E2E_KIND=1`.

1. Provision warehouse cho demo ws; set làm notebook warehouse (`PUT notebook-settings`).
2. Trong notebook (kernel thật), `df = daft.from_pydict({...}); write_iceberg(df,
   "scratch")` → thành công; `back = read_iceberg("scratch")` → trả về dòng (rw trực
   tiếp, không qua engine).
3. **Confinement:** thử ghi table ở namespace KHÁC → bị catalog guard **từ chối**
   (403). Chứng minh token không ghi ra ngoài namespace reserved.
4. Đọc governed vẫn chạy: `trino("SELECT …")` đọc dữ liệu chia sẻ qua engine.

## 5. Luồng dữ liệu

```
admin: PUT /notebook-settings { warehouse_id, namespace }
  └─ provision: create_role("notebook") + add_grant(namespace, "write")   §4.2

kernel start (mỗi user×workspace):
  build_notebook_runtime
    ├─ engine_jwt  (sẵn có, Keycloak refresh — đọc engine governed)
    └─ catalog_jwt = sign_claims({aud:"catalog", warehouse, roles:[notebook]})  §4.3
       + catalog_uri, catalog_warehouse, namespace  → manifest → EGRET_NOTEBOOK_RUNTIME

notebook ghi (scratch riêng):
  write_iceberg(df, "t")
    └─ RestCatalog(uri, token=catalog_jwt, warehouse) → {namespace}.t
       → EgretCatalogGuard: prefix==warehouse ✓ + grant namespace (write) ✓ → OK
       (namespace/table khác → grant miss → 403)

notebook đọc governed:  trino(sql) → JWT per-user → Trino+Ranger  (không đổi)
```

## 6. Edge case, rủi ro & câu hỏi mở (giải khi làm plan)

1. **Tạo NAMESPACE rộng hơn ghi table** (nuance catalog-guard): `POST
   /{prefix}/namespaces` (create) có `target_ns=None` → nhánh catalog-root cho phép
   bất kỳ ai có grant `write`/`manage` (main.rs:368-375). Nên token notebook có thể
   tạo namespace KHÁC (không chỉ ns reserved), dù không ghi được TABLE ngoài ns
   reserved. Giảm thiểu: helper kernel chỉ trỏ ns reserved (không expose tạo
   namespace tùy ý). Ghi chú nuance; confinement DỮ LIỆU (table) là chính xác.
2. **Notebook warehouse phải tồn tại.** Setting cần workspace đã có warehouse (ws sở
   hữu N warehouse user tạo, có thể 0). Nếu chưa set, `catalog_jwt=None` và
   `write_iceberg` báo lỗi rõ. Không auto-provision ở bản này.
3. **TTL / refresh catalog-JWT.** Token TTL ~1h. Kernel có thể sống lâu hơn. Theo
   precedent refresh engine-JWT (`POST /api/ws/{workspace}/notebook-runtime/token`,
   routes.rs:~820) — thêm biến thể catalog, hoặc re-mint on-demand. V1 embed trong
   manifest + ghi TTL; refresh endpoint để sau.
4. **Engine thấy scratch (Q6):** namespace THẤY được từ Trino/StarRocks của ws (cùng
   warehouse/REST), nhưng role catalog của ENGINE cần grant read RIÊNG trên namespace
   để query (2 token authorize độc lập). "Query scratch governed qua engine" là
   nice-to-have — NGOÀI phạm vi v1 (thêm engine-role read grant sau).
5. **Chi tiết daft ↔ pyiceberg** (suy schema, chữ ký `create_table_if_not_exists`,
   write mode). pyiceberg + daft đã cài; verify API + extra (vd `pyiceberg[pyarrow]`)
   khi làm.
6. **Scratch chia sẻ per-workspace** → thành viên ws có thể ghi đè table scratch của
   nhau (1 namespace chung). Chấp nhận theo quyết định per-workspace; per-user
   (`notebook_{user}`) là refinement tương lai.

## 7. Chiến lược test

- **Rust unit** — helper mint catalog-JWT ra đúng claims (`aud:"catalog"`, warehouse
  + role đúng, exp); store setting notebook-warehouse round-trip + validate
  warehouse-thuộc-workspace; provisioning role/grant idempotent.
- **Kernel pytest** — `write_iceberg`/`read_iceberg` dựng RestCatalog với token +
  ép ns reserved (mock pyiceberg/daft như test kernel hiện có mock daft).
- **e2e (kind)** — §4.6: rw round-trip trực tiếp + ghi-ngoài-namespace bị từ chối +
  đọc governed qua engine.
- Chạy đủ CI local trước merge (fmt/clippy/workspace test; FE lint/typecheck/test/
  build; kernel pytest).

## 8. Ngoài phạm vi

- Auto-provision notebook warehouse (admin cấu hình warehouse có sẵn).
- Engine đọc namespace scratch dưới governance (cần engine-role read grant —
  follow-up).
- Namespace scratch per-user (thay vì per-workspace).
- Đổi đọc governed hay đường `write_table` (engine-write) hiện có.
