# B1b-0 — Internal Event Bus + Cedar PDP Hot-Reload

> **Bản tham chiếu tiếng Việt.** Bản chuẩn là [tiếng Anh](../../en/specs/2026-07-10-b1b-0-event-bus-hot-reload-design.md) — khác biệt thì lấy bản EN làm gốc.
>
> **Trạng thái:** Design (đã duyệt để lên plan) · **Ngày:** 2026-07-10 · **Phiên bản:** 1.0
>
> **Doc cha:** [Effort B authorization design](../architecture/2026-07-10-effort-b-authorization-design.md). Đây là increment ĐẦU của **B1b** (PEP cutover), làm trước vì nó gỡ blocker cho phần còn lại.
>
> **Gỡ tiền đề:** finding #3 của security review — `CedarPdp::build` snapshot `roles` + `role_permissions` một lần → sửa RBAC bị stale tới khi restart. Hai authorizer cũ đọc per-request, nên PDP không thể wire vào serving (B1b-1) tới khi nó reload theo thay đổi.

---

## 1. Vì sao

B1a ship một Cedar PDP dormant với template set build một lần lúc construct. Trước khi B1b-1 wire PDP vào xử lý request, nó phải luôn tươi với RBAC console (tạo/xoá role, đổi permission). Kiến trúc service-ready (Effort A §3.3/§4.6) đã plan một **event bus** (`egret-events`) đúng cho lớp tín hiệu async cross-seam này, nhưng chưa build. Ta build nó bây giờ — một transport nhỏ, tái dùng — và wire **một** topic (RBAC đổi → PDP reload) end-to-end vừa làm fix #3 vừa là consumer thật đầu tiên của bus.

Scope hẹp có chủ đích: chỉ build bus + đường roles→reload. Các topic khác đã plan (`workspace.deleted`, `warehouse.provisioned`, `policy.changed`, `catalog.changed`, `query.finished`) là việc tương lai mà bus này bật ra, không làm ở đây.

## 2. `egret-events` — transport

Crate mới `egret-events` cung cấp bus publish/subscribe in-process. "In-proc now, remote adapter later" — bám pattern Port của egret.

### 2.1 Marker event
```rust
/// Domain event. Mỗi seam tự định nghĩa event của mình (không enum chung) để
/// seam giữ isolation — bus route theo type cụ thể.
pub trait Event: Clone + Send + Sync + 'static {}
```

### 2.2 Bus handle
Vì method publish/subscribe **generic theo kiểu event**, bus không thể là `dyn` trait object (method generic không object-safe). Nên nó là **handle cụ thể, clone rẻ** (lựa chọn idiomatic Rust, như chính `tokio::sync::broadcast`). Backend phân tán tương lai là swap nội bộ sau cùng surface này (event khi đó cần thêm `Serialize`; ngoài scope).

```rust
#[derive(Clone, Default)]
pub struct EventBus { inner: Arc<Mutex<HashMap<TypeId, Box<dyn Any + Send + Sync>>>> }

impl EventBus {
    pub fn new() -> Self { Self::default() }
    /// Publish tới mọi subscriber hiện tại của E. No-op nếu không có. Không block.
    pub fn publish<E: Event>(&self, event: E) { /* gửi qua broadcast::Sender<E> */ }
    /// Subscribe event E tương lai.
    pub fn subscribe<E: Event>(&self) -> broadcast::Receiver<E> { /* get-or-create sender rồi .subscribe() */ }
}
```
- Backing `tokio::sync::broadcast` (buffer chặn, vd capacity 64). Fire-and-forget: subscriber chậm nhận `RecvError::Lagged(n)` chứ không block publisher.
- Nội bộ khoá map ngắn, key `TypeId::of::<E>()`, downcast `Box<dyn Any>` → `broadcast::Sender<E>`, tạo on-demand lúc `subscribe` (publish không subscriber = no-op rẻ).

### 2.3 Hợp đồng delivery
Event **best-effort và coalescible**. Consumer phải chịu được tín hiệu mất/lặp: consumer roles→reload coi event nhận được và lỗi `Lagged` như nhau ("có gì đó đổi → rebuild"). Giữ bus đơn giản, reload idempotent.

## 3. Event `RolesChanged` + điểm publish

Định nghĩa trong `egret-auth` (seam sở hữu `roles`/`role_permissions`):
```rust
#[derive(Clone)] pub struct RolesChanged;
impl egret_events::Event for RolesChanged {}
```

`RoleRepo` (`egret-auth/src/roles.rs`) là funnel DUY NHẤT cho mutation role/perm — kể cả handler `set_role_permissions` của control-plane cũng qua nó — nên publish ở đây phủ mọi caller. `RoleRepo` nhận thêm một `EventBus` và publish `RolesChanged` **sau khi ghi thành công** đúng 3 method đổi template set:

| Method | Publish? | Vì sao |
|---|---|---|
| `create(scope, name, description)` | ✅ | role mới → template mới |
| `delete(id)` | ✅ | role mất → bỏ template |
| `set_permissions(id, perms)` | ✅ | action list đổi → thân template đổi |
| `update_description(id, …)` | ❌ | description không nằm trong template |
| `assign` / `unassign` | ❌ | grant assignment đọc per-request qua `AccessGrantStore::grants_for` — không cache trong template |

`RoleRepo::new` nhận `EventBus`. Test và constructor khác truyền một bus (`EventBus::new()` mới nếu không cần subscriber).

**Write out-of-band** (seed, migration, SQL tay) không publish. Chấp nhận được: seed/migration lúc startup chạy trước serving nên `CedarPdp::build` ban đầu bắt; SQL tay runtime hiếm, được change kế tiếp hoặc restart bắt. (Bỏ phương án DB-trigger để tránh đường DB→bus thứ hai; xem lại nếu write RBAC out-of-band runtime thành workflow thật.)

## 4. Hot-reload `CedarPdp`

Restructure để template set nằm sau một cell swap được, và task nền rebuild theo `RolesChanged`.

```rust
struct PdpInner { templates: PolicySet, role_names: HashMap<Uuid, String> }

#[derive(Clone)]
pub struct CedarPdp { grants: AccessGrantStore, db: Db, inner: Arc<ArcSwap<PdpInner>> }

impl CedarPdp {
    pub async fn build(db: Db, bus: &EventBus) -> Result<Self> {
        let inner = Arc::new(ArcSwap::from_pointee(Self::load_inner(&db).await?));
        let pdp = Self { grants: AccessGrantStore::new(db.clone()), db, inner };
        pdp.spawn_reload(bus.subscribe::<RolesChanged>());
        Ok(pdp)
    }
    async fn load_inner(db: &Db) -> Result<PdpInner> { /* thân build() cũ, tách ra */ }
    fn spawn_reload(&self, mut rx: broadcast::Receiver<RolesChanged>) {
        let (db, inner) = (self.db.clone(), self.inner.clone());
        tokio::spawn(async move {
            loop { match rx.recv().await {
                Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => match CedarPdp::load_inner(&db).await {
                    Ok(next) => inner.store(Arc::new(next)),
                    Err(e) => tracing::error!(?e, "PDP reload failed; keeping previous policy set"),
                },
                Err(broadcast::error::RecvError::Closed) => break,
            }}
        });
    }
}
```
- `authorize` đọc `let inner = self.inner.load();` dùng `inner.templates`/`inner.role_names` (còn lại như B1a).
- **Fail-safe:** rebuild lỗi (vd DB blip) → giữ policy set tốt trước đó — PDP không bao giờ tụt về allow-all hay rỗng.
- Dep mới: `arc-swap` (cell lock-free read-mostly). Thay thế `tokio::sync::RwLock<Arc<PdpInner>>` không cần dep; ưu tiên `arc-swap` cho đường `authorize` read-heavy. Plan chọn cái nào cũng được.

## 5. Composition & wiring

- `egret-bin` (composition root) tạo một `EventBus`, lưu vào shared app state, trao cho `RoleRepo` (publisher).
- Bus sẵn cho `CedarPdp::build` subscribe. **Construct `CedarPdp` vào serving state là B1b-1** (khi `require()` dùng nó); B1b-0 không wire PDP dormant vào xử lý request. B1b-0 chứng minh reload bằng integration test: construct `CedarPdp` từ bus, mutate role qua `RoleRepo`, quan sát quyết định đổi.

## 6. Testing

- **egret-events unit:** publish→subscribe giao được; hai kiểu event route độc lập; subscriber vào sau publish thì lỡ (semantics đã ghi); subscriber chậm nhận `Lagged` chứ không block publisher.
- **RoleRepo publish:** `set_permissions`/`create`/`delete` mỗi cái phát đúng một `RolesChanged`; `update_description`/`assign` không phát.
- **Hot-reload integration (regression #3):** `CedarPdp::build(db, &bus)`; assert subject bị deny `action X` ở một scope; qua `RoleRepo` (dùng chung `bus`) grant `X` cho role của subject bằng `set_permissions`; đợi reload (poll `authorize` tới khi flip, timeout chặn); assert giờ allow — **không** construct lại PDP. Assert cả gỡ perm thì flip lại.
- **Full local gate:** `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, và FE gate (không có FE surface ở đây, nhưng chạy theo thông lệ).

## 7. Ranh giới scope

- **Trong:** crate `egret-events` (bus + `Event` + impl in-proc + test); `RolesChanged` + publish trong `RoleRepo`; reload `CedarPdp` qua `ArcSwap` + reload task; tạo bus ở `egret-bin` + thread vào `RoleRepo`; integration test reload.
- **Ngoài:** wire `CedarPdp` vào serving / `require()` (B1b-1); topic event khác; backend bus remote/serialized; đường publish DB-trigger.

## 8. Câu hỏi mở (cho plan)

- `arc-swap` vs `RwLock<Arc<PdpInner>>` — chọn một (mặc định: `arc-swap`).
- Capacity kênh `broadcast` (mặc định 64) — đủ để burst sửa-RBAC bình thường không ép `Lagged` (dù `Lagged` vẫn xử lý đúng).
- Tham số `EventBus` mới của `RoleRepo::new` thread qua mọi call-site hiện tại ngay, hay thêm `RoleRepo::with_bus` cạnh `new` bus-less dùng `EventBus::new()` tách rời (ít sửa call-site). Plan quyết; ưu tiên thread tường minh nếu call-site ít.

---

*Design v1.0 (2026-07-10). Increment đầu của B1b; gỡ blocker wire Cedar PDP vào serving bằng cách cho nó reload theo thay đổi RBAC. Build bus tái dùng `egret-events` (Effort A §3.3), chỉ wire topic roles→reload.*
