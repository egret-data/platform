# Effort B — Hợp nhất Authorization: North-Star + B1

> **Bản tham chiếu tiếng Việt.** Bản chuẩn (canonical) là [tiếng Anh](../../en/architecture/2026-07-10-effort-b-authorization-design.md) — khi có khác biệt, lấy bản tiếng Anh làm gốc.
>
> **Trạng thái:** Design (đã duyệt để lên spec) · **Ngày:** 2026-07-10 · **Phiên bản:** 1.0
>
> **Doc cha:** [Kiến trúc service-ready v1.0](./2026-07-08-service-ready-architecture-design.md) §6. Doc đó chuẩn bị các seam; doc này là thiết kế authorization mà B cắm vào.
>
> **Tiền đề (đã đủ):** E0 (gate auth dời về `egret-auth`, token helper chung), E2 (Policy tách), E3 (Catalog tách).

---

## 1. Vì sao

Egret hôm nay có **hai authorizer song song**, sinh ra từ hai chiến lược resolve khác nhau:

| | `egret-auth::authz` (gate tĩnh) | `egret-workspace::rbac::Authorizer` (gate DB) |
|---|---|---|
| Mô hình | hàm thuần trên `Principal.permissions` nạp-sẵn-lúc-session | query DB mỗi request |
| Resolve | eager (lúc build session) → stale tới khi re-login | lazy (mỗi request) → tươi |
| Dùng bởi | bề mặt platform: policy, warehouse, engine-mgmt, control-plane access, storage, secrets (~80 call-site) | bề mặt workspace: bi, notebook, query, kernel, workspace-members (~120 call-site) |
| API | `require_permission(&p, key)` free fn | `authz.require_workspace(&ctx, key)` method |

Hệ quả: `require_platform` bị **trùng** (tĩnh trên session perms *và* query lại DB trong `Authorizer`) → hai câu trả lời cho cùng câu hỏi; thay đổi platform permission bị stale trong một session; workspace-feature permission (`dashboard:read`, `notebook:*`…) enforce bằng string literal trần và **không có trong catalog trung tâm** (`egret-core::rbac::PERMISSIONS`) nên drift-lint không thấy.

Effort B hợp nhất thành một mô hình quyết định duy nhất, đồng thời mang object-scope, delegation data-plane per-user, và audit thật — nhưng chia thành **các sub-epic riêng** (xem §5). Doc này chốt north-star và chi tiết **B1** (nền tảng).

## 2. North-star: authorization phân tầng

Phân tầng chuẩn hiện đại (thuật ngữ NIST/XACML: PDP/PEP/PAP/PIP + grant quan hệ kiểu Zanzibar-lite), hiện thực in-process với **Cedar** (`cedar-policy`, AWS, Apache-2.0) làm engine quyết định. Tái sử dụng theo thiết kế: **thêm feature = khai action trong catalog + chọn scope-level; PDP/PEP không đổi.**

| Tầng | Vai trò | Hôm nay | Đích |
|---|---|---|---|
| **L0 — Identity (AuthN)** | OIDC + local + machine token → `Subject` (human/service) + group membership | oidc, session, groups (đã có) | B3 mở rộng machine/delegation token |
| **L1 — Grant store (PAP)** | Nguồn sự thật "ai-có-gì-ở-đâu": grant tuple `(principal, role, scope)`, `scope ∈ {platform, workspace:{id}, object:{type,id}}`. Role = bó permission. Group = expand thành viên. | 2 bảng tách rời | **B1** gộp thành 1 bảng `access_grants` (object-scope để dành B2) |
| **L2 — PDP (Decision)** | `authorize(subject, action, resource) -> Decision`: resolve grant dọc chuỗi scope (object → workspace → platform), evaluate policy Cedar (permit/**forbid**), trả allow/deny + policy đóng góp. **1 chokepoint.** | 2 authorizer | **B1** dựng (Cedar sau Port) |
| **L3 — PEP (Enforce)** | Middleware dựng `AuthContext`; handler gọi `require(ctx, action[, resource])`. Mỏng, đồng nhất. | 2 style gọi | **B1** thống nhất |
| **L4 — Data-plane federation** | Grant + policy control-plane compile xuống Ranger policy + per-user downscoped token cho engine/catalog. Cùng subject & grant model. | Ranger + machine-JWT (rời) | B3 + epic Policies sẵn có |
| **Cross-cutting — Audit** | Mọi quyết định PDP + mọi grant-mutation → `audit_log`. Diagnostics Cedar cấp lý do. | chỉ `policy_audit` cục bộ | **B4** |

### 2.1 Vì sao Cedar (không phải Oso)

- **Thư viện OSS của Oso đã sunset** (pivot sang Oso Cloud dạng service) → rủi ro maintenance cho phụ thuộc in-process dài hạn.
- **Cedar là Rust-native, AWS phát triển tích cực** (nền của AWS Verified Permissions); evaluate hoàn toàn **in-process** → giữ nguyên tắc 1-binary của egret.
- **`forbid` override `permit`** (deny thắng) → explicit-deny và Ranger-parity (B2) có sẵn miễn phí.
- **Entity hierarchy** (toán tử `in`) map thẳng chuỗi scope object → workspace → platform.
- **Schema + validation** và **policy-as-data** (lưu DB, hot-reload qua event `policy.changed` đã kế hoạch).

### 2.2 Mô hình Cedar cho egret

- **Entities (dữ liệu, dựng per-request từ grant store):**
  - Principal: `User::"<uuid>"`, `Service::"<id>"` (catalog client, kernel). `User in [Group::"…", Role::"…"]`.
  - Resource: `Platform::"root"`, `Workspace::"<id>"`, và (B2) object có kiểu `Dashboard::"<id>"`, `Warehouse::"<id>"`… với hierarchy `Object in Workspace in Platform`.
  - `Role` mang attribute `permissions: Set<String>` (từ `role_permissions`), scope qua `Role in [Workspace | Platform]`.
- **Actions** = catalog permission hợp nhất (control-plane từ `egret-core::rbac` **+** workspace-feature actions gom vào). Schema khai `appliesTo` principal/resource types.
- **Policy-set tĩnh** (ít, versioned; grant là *data* entity, không phải policy per-user — Zanzibar-lite):
  - `permit(principal in Role::"platform_admin", action, resource);` — bypass.
  - platform: `permit` khi principal giữ action qua platform-role và `resource == Platform::"root"`.
  - workspace: `permit` khi principal giữ action qua role scoped vào workspace của request và `resource in Workspace::"…"`.
- **Decision** trả `Allow/Deny` + id policy đóng góp → lý do audit (B4).

## 3. B1 — Hợp nhất authorizer trên Cedar (behavior-preserving)

**Mục tiêu:** một Cedar PDP thay cả hai authorizer, **không đổi hành vi** (mọi gate cho cùng allow/deny); **không** object-scope (B2), **không** audit content (B4) — nhưng `resource` và diagnostics đã đi dây sẵn. Theo scope đã duyệt, B1 **gộp luôn bảng grant** (L1) để data-model sạch trước khi object-scope đến.

### 3.1 L1 — bảng grant hợp nhất

Bảng mới (schema đích `iam`), thay `platform_role_assignments` và `workspace_members`:

```sql
CREATE TABLE access_grants (
    id             uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    principal_type text NOT NULL CHECK (principal_type IN ('user','group','service')),
    principal_id   uuid NOT NULL,
    role_id        uuid NOT NULL REFERENCES roles(id) ON DELETE CASCADE,
    scope_type     text NOT NULL CHECK (scope_type IN ('platform','workspace','object')),
    scope_id       uuid,          -- NULL cho platform; workspace_id cho workspace; object id cho B2
    object_type    text,          -- NULL ở B1; để dành B2
    granted_by     uuid,
    created_at     timestamptz NOT NULL DEFAULT now(),
    UNIQUE (principal_type, principal_id, role_id, scope_type, scope_id)
);
```

- `platform_role_assignments` → `scope_type='platform', scope_id=NULL`.
- `workspace_members (…, role)` → `scope_type='workspace', scope_id=workspace_id, role_id=lookup(roles WHERE scope='workspace' AND name=role)`.
- `roles`, `role_permissions`, `user_groups`, `groups` giữ nguyên.
- **Migration** back-fill `access_grants` từ hai bảng rồi drop chúng. Liệt kê workspace-member thành `WHERE scope_type='workspace' AND scope_id=$ws`. Suy ra "primary role" (role ưu tiên cao nhất) để giữ `WorkspaceContext.workspace_role` cho hiển thị.

### 3.2 L2 — PDP sau Port

- Trait mới trong `egret-core` (crate identity-only, engine-agnostic):
  ```rust
  pub struct Decision { pub allow: bool, pub reasons: Vec<String> }
  #[async_trait] pub trait Pdp {
      async fn authorize(&self, subject: &Subject, action: &str, resource: &Resource) -> Decision;
  }
  ```
  `Resource` là `Platform | Workspace(WorkspaceId)` ở B1; thêm variant object ở B2 mà không đụng call-site.
- Impl Cedar nằm trong `egret-auth` (module `authz/`): sở hữu `.cedarschema`, policy-set `.cedar` tĩnh, `Authorizer`, entity provider. **Handler & PEP không tham chiếu Cedar** — chỉ port `Pdp`.

### 3.3 L2 — entity provider

Thay `session::permissions` + `rbac::Authorizer`. Từ `access_grants` + `role_permissions` + `user_groups` dựng entity-set Cedar cho `(subject, chuỗi-scope-resource)`, memo per-request. Một dạng query phủ cả platform lẫn workspace (lọc theo scope), xoá bỏ chia eager/lazy.

### 3.4 L3 — PEP hợp nhất

- Một helper `require(ctx, action[, resource])` thay **cả** `require_permission(&p, key)` (~80 site) **lẫn** `authz.require_workspace/require_platform(&ctx, key)` (~120 site).
- `WorkspaceContext` trở thành/mở rộng `AuthContext` mang subject + scope của request. Middleware dựng nó (route platform → scope `Platform`; route workspace → `Workspace(id)`, như hôm nay).

### 3.5 Catalog hợp nhất

Gom workspace-feature actions (`dataset:*`, `chart:*`, `dashboard:*`, `notebook:*`, `query:*`, `kernel:*`…) vào `egret-core::rbac::PERMISSIONS`, mỗi cái gắn scope-level. Đóng gap catalog để drift-lint và Cedar schema thấy đủ mọi action. Schema Cedar sinh/validate theo catalog này.

### 3.6 Giữ nguyên hành vi

- Giữ lint `catalog_drift`; mở rộng tới call-site workspace-feature.
- **Regression matrix test:** với tập đại diện (role, action, scope), khẳng định Cedar PDP trả cùng allow/deny như logic trước B1 (bảng golden).
- Full-workspace `fmt + clippy + test` **và** frontend `lint + typecheck + test + build` xanh cục bộ trước merge (GitHub CI hết quota — theo thông lệ project).

### 3.7 Ranh giới B1 (YAGNI)

- **Trong:** Cedar PDP + Port, bảng `access_grants` hợp nhất + migration, PEP + catalog hợp nhất, regression behavior-preserving.
- **Ngoài:** object-level grant / role hierarchy (B2), `audit_log` content (B4), delegation catalog/engine per-user + Ranger federation (B3/L4). Enum `Resource`, cột `scope_id`/`object_type`, và `reasons` là các seam để chúng cắm vào.

## 4. Giao diện & cô lập

- `egret-core`: `Subject`, `Resource`, `Decision`, trait `Pdp`, catalog `PERMISSIONS` hợp nhất. Không phụ thuộc engine.
- `egret-auth`: crate DUY NHẤT biết Cedar tồn tại — schema, policy, entity provider, impl `Pdp`, middleware dựng `AuthContext`, helper `require()`.
- Mọi seam khác chỉ phụ thuộc `egret-core` (`require`, `Pdp`) — chiều phụ thuộc không đổi; seam vẫn gate mà không kéo controlplane hay Cedar.

## 5. Chuỗi sub-epic

| Sub-epic | Nội dung | Phụ thuộc |
|---|---|---|
| **B1 — Hợp nhất authorizer** (§3) | Cedar PDP + Port, `access_grants` hợp nhất, PEP + catalog hợp nhất, behavior-preserving | E0/E2/E3 (đã đủ) |
| **B2 — Object-level scoping** | object grant + variant object của `Resource` + kế thừa hierarchy + explicit deny; hợp nhất UI share object (collection kiểu Metabase) | B1 |
| **B3 — Delegation data-plane per-user** | thay machine-JWT bằng per-user downscoped token (catalog + Trino + StarRocks); federate grant control-plane vào Ranger | B1, token helper E0, catalog contract E3 |
| **B4 — `audit_log` thật** | middleware ghi mọi quyết định + grant-mutation từ diagnostics Cedar; schema `audit` | B1 (chokepoint) |

## 6. Câu hỏi mở (cho plan B1)

- Versioning schema/entity Cedar + đi dây hot-reload với event `policy.changed` (đã kế hoạch), hay static-embed ở B1 và hoãn reload sang B2.
- Quy tắc "primary role" chính xác để giữ hiển thị `WorkspaceContext.workspace_role`.
- `Service` principal (catalog client, kernel) có đi qua cùng PDP ở B1 không, hay giữ token check hiện tại tới B3.

---

*Design v1.0 (2026-07-10). North-star cho Effort B; B1 là increment đầu. Companion của Kiến trúc service-ready v1.0.*
