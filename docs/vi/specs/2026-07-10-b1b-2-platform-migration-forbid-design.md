# B1b-2 — Migrate platform seams + mô hình admin-graph `forbid`

> **Bản tham chiếu tiếng Việt.** Bản chuẩn là [tiếng Anh](../../en/specs/2026-07-10-b1b-2-platform-migration-forbid-design.md) — khác biệt lấy bản EN làm gốc.
>
> **Trạng thái:** Design (đã duyệt để lên plan) · **Ngày:** 2026-07-10 · **Phiên bản:** 1.0
>
> **Tiền nhiệm:** [B1b-1 PDP wiring + require()](./2026-07-10-b1b-1-pdp-wiring-require-design.md).
>
> Increment thứ ba của B1b. Migrate mọi platform seam còn lại khỏi hai gate tĩnh cũ (`require_permission` và `require_platform_admin`) sang `require()` backed bởi PDP, và model guardrail admin-only thành một Cedar `forbid` policy — biến một invariant "chuỗi không bao giờ được grant" mong manh thành đảm bảo cấu trúc.

---

## 1. Vì sao & scope

B1b-1 wire PDP + migrate seam `secrets`. B1b-2 hoàn tất tầng platform: ~80 site `require_permission` (access, storage, engine-mgmt, warehouse-admin, policy-admin) + 11 site `require_platform_admin` trên các route sửa RBAC-graph. Phần cơ học là pattern B1b-1 (inject `Arc<dyn Pdp>` vào state; swap call). Phần không-cơ-học — lý do đây là design chứ không chỉ là chore — là **model admin-only qua PDP cho đúng** (§2).

**Trong scope:** mô hình admin-graph `forbid`; migrate mọi site platform `require_permission` + `require_platform_admin` sang `require()`; inject PDP vào `EngineMgmtState`, `CatalogAdminState`, `PolicyAdminState` (access/storage đã có qua `CpState`).
**Ngoài scope:** workspace seams (B1b-3); xoá primitive cũ đã chết (`require_permission`, `require_platform_admin`) + `Principal.permissions` (B1b-final, khi hết caller); `Principal.platform_roles` + `is_platform_admin` (giữ — còn dùng bởi workspace-context extractor).

## 2. Mô hình admin-graph `forbid` (security-critical — review kỹ mục này)

### 2.1 Vấn đề của cách ngây thơ
Authorization có hai trục khác bản chất mà code hiện tại trộn:
1. **Permission (RBAC):** "subject làm được action X?" — *delegatable* qua role.
2. **Guardrail admin-graph:** "chỉ `platform_admin` được sửa chính đồ thị phân quyền (tạo/xoá role, gán permission/role, đổi group membership)." — cố ý **non-delegatable**; đây là fix privilege-escalation PR #174.

Hôm nay trục 2 xử lý bằng cách xoá action khỏi catalog + gate route bằng `require_platform_admin` — một authorizer thứ ba vô hình. Route qua PDP như action thường sẽ khiến chúng *delegatable* (grant cho role → non-admin có), phá guardrail. Chặn bằng check runtime "chuỗi không bao giờ vào `role_permissions`" là **negative invariant** — mong manh đúng chỗ quan trọng nhất.

### 2.2 Mô hình: một static `forbid` policy
Cedar `forbid` **override** mọi `permit`. Guardrail thành cấu trúc:
```cedar
// Admin-graph actions chỉ cho platform_admin. forbid thắng mọi permit,
// nên không grant nào — vô tình hay cố ý — override được.
forbid(
  principal,
  action in [Egret::Action::"role:create", Egret::Action::"role:update",
             Egret::Action::"role:delete", Egret::Action::"role:assign_permission",
             Egret::Action::"user:assign_role", Egret::Action::"group:manage_members"],
  resource
)
unless { context has is_platform_admin && context.is_platform_admin };
```
- **Non-escalatable by construction:** kể cả khi `role:create` bị grant cho role non-admin, `forbid` vẫn deny. Bảo vệ escalation không còn phụ thuộc catalog hygiene.
- **`unless { context has is_platform_admin && context.is_platform_admin }`** là **fail-safe**: nếu thiếu flag trong context, `has` false → `unless` false → `forbid` **áp dụng** (deny). Request méo chỉ có thể fail-closed.
- Danh sách action là const Rust `ADMIN_GRAPH_ACTIONS: &[&str]` trong `egret-core::rbac`, và chuỗi `forbid` sinh ra từ nó (single source of truth). Danh sách = tập action hiện gated bởi `require_platform_admin` (liệt kê per-route lúc plan bằng cách đọc `access/mod.rs`).

### 2.3 Cấp `is_platform_admin`
`authorize()` của PDP đã load grants của subject (`grants_for`) + map `role_id → name`. Nó tính:
```rust
let is_admin = grants.iter().any(|g| g.scope_type == "platform"
    && inner.role_names.get(&g.role_id).map(String::as_str) == Some("platform_admin"));
```
và truyền `Context::from_json_value(json!({ "is_platform_admin": is_admin }))` vào Cedar `Request` (thay `Context::empty()`). Tái dùng đúng tín hiệu admin mà template permit platform_admin vốn key vào — không nguồn sự thật mới, không restructure entity.
(Phương án khác: thêm cạnh entity `User in Role::"platform_admin"` trong `build_entities` + `unless { principal in Role::"platform_admin" }`. Cedar-idiomatic hơn nhưng invasive hơn vào code entity B1a; context flag dễ review hơn cho gate bảo mật. Cả hai đúng; design này dùng context flag.)

### 2.4 Static policy trong `load_inner`
`CedarPdp::load_inner` (B1b-0) build `PolicySet` các linked template per-role. Giờ **thêm** static `forbid` policy (sinh từ `ADMIN_GRAPH_ACTIONS`) một lần. Là hằng số nên re-add mỗi lần hot-reload rebuild — không staleness. Là `Policy` thường (không phải template), parse + `add` vào set.

### 2.5 Xử lý catalog (defense in depth)
`ADMIN_GRAPH_ACTIONS` được PDP nhận diện nhưng **KHÔNG** vào grantable catalog phơi ra bởi console (`list_permissions`), và `set_role_permissions`/`RoleRepo::set_permissions` vẫn từ chối chúng (không phải grantable key `is_known`). Vậy có **hai** tầng độc lập: không offer/insert được làm role permission (hygiene), và kể cả có insert thì `forbid` deny (structural). `forbid` là đảm bảo; catalog-exclusion là "đừng cả offer nó".

### 2.6 Migrate 11 site & test
Mỗi site `require_platform_admin(&p)?` thành `require(&*st.pdp, &p, "<action admin-graph của route đó>").await?` — dùng action riêng cho route (vd route create-role → `"role:create"`), auditable hơn check admin chung, đồng nhất với mọi gate khác. Không thêm helper `require_admin`.

**Test behavior-preservation (bắt buộc):**
- `platform_admin` được phép mọi admin-graph action.
- non-admin giữ **mọi** grantable catalog permission (và adversarially, kể cả role bị force-insert `role:create` qua raw SQL) bị **deny** mọi admin-graph action.
- non-admin (context `is_platform_admin=false`) trả `Error::Forbidden` → 403 (như legacy).
- Mở rộng regression matrix 60-cell (hoặc thêm matrix riêng) gồm admin-graph action × {admin, non-admin} và assert kết quả `forbid`.

## 3. Migrate per-seam (cơ học, pattern đã chốt)

Mỗi seam: thêm `pub pdp: Arc<dyn egret_core::authz::Pdp>` vào state (nơi thiếu), inject PDP chung từ `egret-bin`, thay mỗi `require_permission(&p, "key")?` bằng `egret_core::authz::require(&*st.pdp, &p, "key").await?` (chỉnh tên biến state/principal theo handler), sửa test của seam đó dùng real `CedarPdp` nơi assert authz (seed role **trước** khi build PDP, vì raw-SQL seed không phát `RolesChanged`). Behavior giữ vì quyết định Platform của PDP == quyết định `require_permission` legacy (regression matrix B1a).

| Seam | File | ~site | State | Inject |
|---|---|---|---|---|
| access | `egret-controlplane/src/access/mod.rs` | 13 `require_permission` + 11 `require_platform_admin` | `CpState` | đã có `pdp` |
| storage | `egret-controlplane/src/storage.rs` | 8 | `CpState` | đã có `pdp` |
| engine-mgmt | `egret-engine-mgmt/src/{engines,monitor}.rs` | 19 | `EngineMgmtState` | **thêm + wire** |
| warehouse-admin | `egret-catalog-engine/src/warehouse_admin/mod.rs` | 28 | `CatalogAdminState` | **thêm + wire** |
| policy-admin | `egret-policy/src/admin/mod.rs` | 12 | `PolicyAdminState` | **thêm + wire** |

## 4. Ranh giới scope

- Primitive cũ `egret_auth::{require_permission, require_platform_admin}` + wrapper `egret_controlplane::auth` thành **chết** sau increment này (không còn caller platform; workspace seams vẫn dùng workspace `Authorizer`). **Không** xoá ở đây — đó là B1b-final, cùng việc bỏ `Principal.permissions` + flip grant-writer. Để chết-nhưng-còn giữ PR này thuần migration.
- `Principal.platform_roles` + `WorkspaceContext::is_platform_admin` giữ (dùng bởi workspace extractor).
- `catalog_drift` lint đã nhận `authz::require(` (B1b-1). Xác nhận admin-graph action được thấy là enforced (giờ xuất hiện dạng `require(..., "role:create")`) và không bị flag là catalog entry unenforced — vì chúng KHÔNG trong grantable catalog nên lint không nên kỳ vọng chúng ở đó; giữ lint honest.

## 5. Testing

- Test admin-graph forbid unit/integration (§2.6).
- Mỗi seam migrate: route test cũ xanh (cùng allow/deny) + một assertion authz real-PDP.
- Full local gate: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, FE `lint/typecheck/test/build`.

## 6. Câu hỏi mở (cho plan)

- Tên action per-route chính xác cho 11 site `require_platform_admin` — liệt kê bằng đọc `access/mod.rs` (create/update/delete role, set-permissions, assign/unassign user role, add/remove group member, replace-platform-roles). Vài cái có thể chia sẻ action.
- Admin-graph action có nên xuất hiện (read-only, xám) trong console để minh bạch, hay ẩn hẳn — mặc định ẩn (như hôm nay); xem lại ở UX pass.
- Shape `Context::from_json_value` + xác nhận Cedar 4.11.2 eval `context has is_platform_admin` như ý (spike B1a xác nhận core API; verify toán tử `has` trên context bằng một test nhanh).

---

*Design v1.0 (2026-07-10). Migrate tầng platform sang PDP `require()` và thay guardrail `require_platform_admin` bằng Cedar `forbid` policy — đảm bảo admin-graph structural, non-escalatable, đồng thời đặt nền deny-override cho B2. Xoá primitive legacy hoãn tới B1b-final.*
