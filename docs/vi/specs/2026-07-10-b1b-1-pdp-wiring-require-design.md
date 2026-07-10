# B1b-1 — Wire Cedar PDP vào serving + `require()` thống nhất

> **Bản tham chiếu tiếng Việt.** Bản chuẩn là [tiếng Anh](../../en/specs/2026-07-10-b1b-1-pdp-wiring-require-design.md) — khác biệt lấy bản EN làm gốc.
>
> **Trạng thái:** Design (đã duyệt để lên plan) · **Ngày:** 2026-07-10 · **Phiên bản:** 1.0
>
> **Doc cha:** [Effort B authorization design](../architecture/2026-07-10-effort-b-authorization-design.md) · **Tiền nhiệm:** [B1b-0 event bus + hot-reload](./2026-07-10-b1b-0-event-bus-hot-reload-design.md).
>
> Increment thứ hai của B1b (PEP cutover). Wire Cedar PDP (giờ đã hot-reload) vào app đang chạy sau một PEP `require()` duy nhất, migrate một seam nhỏ làm proof. Migrate hàng loạt call-site và bỏ legacy là các increment sau.

---

## 1. Vì sao & scope

B1a build PDP; B1b-0 cho nó hot-reload để lên live an toàn. B1b-1 dựng đường enforcement: construct PDP trong app (subscribe **cùng** event bus — review finding F1), giới thiệu một `require()` mà cả platform lẫn workspace handler đều gọi, và migrate seam `secrets` khỏi gate tĩnh cũ làm proof end-to-end.

**Trong scope:** PDP vào `AppState` (cùng bus + F1 e2e test); `Principal.group_ids`; `egret_core::authz::require()` + `AuthTarget` (cả hai overload `Principal`→Platform và `WorkspaceContext`→Workspace, unit-test); migrate seam `secrets`.
**Ngoài scope:** migrate seam khác (B1b-2 platform, B1b-3 workspace); bỏ 2 authorizer cũ hay `Principal.permissions`; flip writers / drop bảng legacy (B1b-final).

## 2. Wire PDP vào `AppState` (đóng F1)

- `egret-bin` đã construct một `EventBus` và trao cho `RoleRepo` (B1b-0). Giờ build luôn PDP từ **cùng** bus đó: `let pdp: Arc<dyn Pdp> = Arc::new(CedarPdp::build(db.clone(), &bus).await?);` — dùng chung instance mới khiến hot-reload thực sự fire (một `EventBus` KHÁC sẽ im lặng không reload — F1).
- `AppState` (trong `egret-server`) thêm field `pdp: Arc<dyn egret_core::authz::Pdp>` + impl `FromRef<AppState>` để handler mọi seam extract được. Seam chỉ phụ thuộc trait `Pdp` của egret-core — không bao giờ phụ thuộc `CedarPdp` hay nội bộ Cedar của egret-auth.
- **F1 regression test** (DB-gated, app-level): dựng `AppState` (hoặc lát tối thiểu: một `bus` chung, `RoleRepo::new(db, bus.clone())`, `CedarPdp::build(db, &bus)` lưu làm app pdp); grant permission qua *chính* `RoleRepo` đó; assert app pdp phản ánh sau reload. Chứng minh wiring thread cùng bus + chống tái vỡ về sau.

## 3. `Principal.group_ids`

`Subject` (input PDP) cần group membership của caller cho Cedar group path. Thêm `group_ids: Vec<GroupId>` vào `egret_core::Principal`, nạp trong `SessionService::validate` (đã nạp `permissions` từ query join `user_groups` — nạp group id cùng chỗ). `Subject` khi đó dựng thuần in-memory từ `Principal`, không query thêm mỗi lần authz.

## 4. `require()` — PEP duy nhất

Trong `egret_core::authz` (cùng chỗ `Subject`/`Resource`/`Decision`/`Pdp`):
```rust
/// Request context resolve ra PDP subject + resource scope của action.
pub trait AuthTarget {
    fn subject(&self) -> Subject;
    fn resource(&self) -> Resource;
}
impl AuthTarget for Principal {
    fn subject(&self) -> Subject { Subject { user_id: self.user_id, group_ids: self.group_ids.clone() } }
    fn resource(&self) -> Resource { Resource::Platform }
}
impl AuthTarget for WorkspaceContext {
    fn subject(&self) -> Subject { self.principal.subject() }
    fn resource(&self) -> Resource { Resource::Workspace(self.workspace_id) }
}
/// Enforce `action` cho `target`. Ok(()) nếu allow, Err(Error::Forbidden) nếu không.
pub async fn require(pdp: &dyn Pdp, target: &impl AuthTarget, action: &str) -> crate::Result<()> {
    if pdp.authorize(&target.subject(), action, &target.resource()).await?.allow { Ok(()) }
    else { Err(crate::Error::Forbidden) }
}
```
- **Migrate platform call:** `require_permission(&p, "engine:create")?` → `require(&*st.pdp, &p, "engine:create").await?`.
- **Migrate workspace call:** `st.authz.require_workspace(&ctx, "dashboard:read").await?` → `require(&*st.pdp, &ctx, "dashboard:read").await?`.
- **Behavior-preserving:** regression matrix 60-cell của B1a đã chứng minh quyết định Platform/Workspace của PDP == quyết định platform/workspace legacy. `require()` trả cùng `Error::Forbidden` khi deny → HTTP mapping không đổi.
- **Chi phí:** platform check giờ có một query `grants_for` (trước là in-memory trên `Principal.permissions` cache session); workspace path vốn đã query per-request. Query indexed nhẹ; memo per-request là tối ưu sau, không bắt buộc ở đây.

## 5. Proof migration — seam `secrets`

`egret-controlplane/src/secrets.rs` gate ~4 handler trên `require_permission(&p, "secret:{read,create,update,delete}")`. Migrate mỗi cái sang `require(&*st.pdp, &principal, "secret:…").await?`. Handler đã extract `Principal` (qua `Auth`) và state mang pdp. Không đổi route/response. Test secrets hiện tại phải vẫn pass (cùng allow/deny); thêm test assert principal non-`platform_admin` giữ `secret:read` (qua platform role) được read và bị deny create — chạy đường PDP, không phải gate tĩnh cũ.

Cả hai overload `AuthTarget` được unit-test trong increment này (`Principal`→`Platform`; `WorkspaceContext`→`Workspace(id)` và subject mang group id của principal), dù chỉ overload platform áp vào route ở đây — overload workspace áp ở B1b-3.

## 6. Testing

- Unit `AuthTarget`/`require` (egret-core): cả hai overload map đúng `Subject`/`Resource`; `require` trả `Ok` khi allow, `Error::Forbidden` khi deny (với `Pdp` stub).
- `Principal.group_ids` nạp đủ: session của user thuộc N group phơi ra đúng group id.
- Seam secrets: test cũ xanh; test mới đường-PDP allow/deny.
- **F1 app-wiring e2e** (§2).
- Full local gate: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, FE `lint/typecheck/test/build`.

## 7. Câu hỏi mở (cho plan)

- `AppState` giữ `Arc<dyn Pdp>` — xác nhận ergonomics `FromRef` và `egret-server` gọi tên được trait (nó phụ thuộc `egret-core`).
- `SessionService::validate` nạp group id trong query `permissions` sẵn (một round-trip) hay query thứ hai — ưu tiên gộp vào join sẵn.
- Xác nhận handler `secrets.rs` đều có `Principal` trong scope tên `principal`/`p` (có, theo audit catalog B1a); chỉnh tên biến theo từng site khi migrate.

---

*Design v1.0 (2026-07-10). Wire Cedar PDP hot-reload vào serving sau một PEP `require()` và chứng minh trên seam secrets; wiring cùng-bus đóng review finding F1. Migrate hàng loạt (B1b-2/3) và bỏ legacy (B1b-final) theo sau.*
