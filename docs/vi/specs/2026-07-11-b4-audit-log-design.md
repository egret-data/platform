# B4 — Real audit_log

> **Bản tham chiếu tiếng Việt.** Bản chuẩn là [tiếng Anh](../../en/specs/2026-07-11-b4-audit-log-design.md) — khác biệt lấy bản EN làm gốc.
>
> **Trạng thái:** Design (đã duyệt để lên plan) · **Ngày:** 2026-07-11 · **Phiên bản:** 1.0
>
> **Xây trên:** B1a–B1b (chokepoint Cedar PDP + `Decision{allow, reasons}`), B2-flip (mọi grant mutation giờ đi qua writers của `access_grants`).
>
> Increment Effort B cuối. Ghi mọi PDP decision (allow + deny) + mọi grant mutation vào schema `audit` riêng, platform admin truy vấn được. Đóng mảng cross-cutting "real audit" mà north-star để dành cho B4.

---

## 1. Mục tiêu & scope

Cedar PDP là chokepoint enforcement duy nhất (`authorize(subject, action, resource) -> Decision{allow, reasons}`, mọi `require`/`require_object` đi qua), `reasons` đã mang Cedar policy id đóng góp. B2-flip cho `access_grants` là kho grant duy nhất → mọi thay đổi grant đi qua vài writer. B4 biến cả hai thành **audit trail append-only**: một `AuditingPdp` decorator bắt mọi decision (zero churn ở PEP site); các grant-mutation handler emit mutation event; cả hai vào một async best-effort sink batch-insert vào `audit.audit_log`. Query API platform-admin đọc lại.

**Trong scope:** schema `audit` + bảng `audit_log`; async `AuditSink` (bounded channel + background batch writer, best-effort drop khi overload); `AuditingPdp` decorator quanh `CedarPdp` (bắt decisions); emit mutation event ở ~8 grant-mutation handler; background retention prune; `GET /control-plane/audit` filter/paginate (platform-admin only); config.

**Ngoài scope:** trang frontend xem audit (chỉ backend + query API — trang để increment sau); giao nhận đảm bảo/durable (user chọn best-effort — rớt khi overload chấp nhận); ship sang SIEM ngoài; audit raw SQL/migration ghi `access_grants` (chỉ app mutation); audit query data-plane (Trino/Ranger có log riêng).

**Quyết định chốt khi brainstorm:** capture **mutations + denies + allows** (full trail); **tất cả async best-effort** qua một buffered channel + background batch writer (không block/fail request; drop khi tràn); **chỉ backend + query API** (no UI); mutation **emit tại grant-mutation handler** (không DB trigger) để một channel đồng nhất mang cả 2 lớp + emit có actor + before/after; sự kiện **system/không-request** dùng `actor_user_id = NULL` + `detail.source = "system"`.

## 2. Schema (`audit` schema)

Migration mới (repo đã qua 0075):

```sql
CREATE SCHEMA IF NOT EXISTS audit;

CREATE TABLE audit.audit_log (
    id            bigserial PRIMARY KEY,
    ts            timestamptz NOT NULL DEFAULT now(),
    kind          text NOT NULL CHECK (kind IN ('decision','mutation')),
    actor_user_id uuid,                       -- NULL cho system/non-request
    workspace_id  uuid,                       -- NULL cho platform-scope
    action        text NOT NULL,              -- vd 'dashboard:read', 'grant:create'
    resource_type text NOT NULL,              -- 'platform'|'workspace'|'object'|'grant'|...
    resource_id   text,                       -- object id / scope id (text — object id là "kind:uuid")
    decision      text CHECK (decision IN ('allow','deny')),  -- NULL cho mutation
    reasons       text[],                     -- Cedar policy id đóng góp (decision)
    detail        jsonb NOT NULL DEFAULT '{}' -- before/after cho mutation; context cho decision
);

CREATE INDEX audit_log_ts_idx     ON audit.audit_log (ts DESC);
CREATE INDEX audit_log_actor_idx  ON audit.audit_log (actor_user_id, ts DESC);
CREATE INDEX audit_log_action_idx ON audit.audit_log (action, ts DESC);
```

Append-only: không gì UPDATE/DELETE ngoài retention prune (§6). Bảng dead `policy_audit` (mig 0007) để nguyên (legacy artifact riêng).

## 3. Async sink (best-effort)

`egret-audit` (crate mới, hoặc module trong egret-core) định nghĩa:

```rust
pub enum AuditEvent {
    Decision { actor: Option<Uuid>, workspace_id: Option<Uuid>, action: String,
               resource_type: String, resource_id: Option<String>, allow: bool,
               reasons: Vec<String> },
    Mutation { actor: Option<Uuid>, workspace_id: Option<Uuid>, action: String,
               resource_type: String, resource_id: Option<String>, detail: serde_json::Value },
}

#[derive(Clone)]
pub struct AuditSink { tx: tokio::sync::mpsc::Sender<AuditEvent> }

impl AuditSink {
    /// Non-blocking: channel đầy → drop event (best-effort) + tăng counter/warn —
    /// không bao giờ block/fail caller.
    pub fn emit(&self, ev: AuditEvent) { let _ = self.tx.try_send(ev); }
}
```

Một background writer task giữ `Receiver`, gom tới `batch_size` event (hoặc `flush_interval_ms`) rồi 1 INSERT multi-row vào `audit.audit_log`. Shutdown thì drain channel. Counter drop được log định kỳ (observability overload). `emit` = `try_send` — request path không await DB write.

## 4. Decisions — `AuditingPdp` decorator

`egret-auth` thêm `AuditingPdp { inner: Arc<dyn Pdp>, sink: AuditSink }` impl `Pdp`:

```rust
#[async_trait]
impl Pdp for AuditingPdp {
    async fn authorize(&self, subject: &Subject, action: &str, resource: &Resource) -> Result<Decision> {
        let decision = self.inner.authorize(subject, action, resource).await?;
        self.sink.emit(AuditEvent::Decision {
            actor: Some(subject.user_id.0),
            workspace_id: resource.workspace_id(),   // helper: Workspace/Object → Some, Platform → None
            action: action.to_string(),
            resource_type: resource.type_str(),      // "platform"|"workspace"|"object"
            resource_id: resource.id_str(),          // ws uuid / "kind:uuid" / None
            allow: decision.allow,
            reasons: decision.reasons.clone(),
        });
        Ok(decision)
    }
}
```

egret-bin bọc `CedarPdp` thật trong `AuditingPdp` trước khi đặt sau `Arc<dyn Pdp>` mà mọi state giữ. Vì mọi `require`/`require_object` gọi `pdp.authorize`, cái này bắt **mọi enforcement decision** (allow + deny) với **zero thay đổi 187 PEP site**. (Code non-enforcement — FE `/me` gating, admission `has_object_grant_in_workspace` — không gọi `authorize` nên đúng là không bị decision-audit.)

## 5. Mutations — emit tại grant-mutation handler

Các handler đổi `access_grants` (post-B2-flip) emit `Mutation` event vào cùng sink sau mutation thành công. Actor = `ctx.principal.user_id` (không nhận từ client). Site (~8):
- `egret-controlplane` access console: role `assign`/`unassign`, `replace_platform_roles`, (role create/update/delete + set_permissions nếu audit cả role-def), member `add`/`remove`.
- `egret-bi` shares: `grant_put` (allow/deny/Blocked), `grant_delete`.

`action` = verb mutation (`grant:create`, `grant:revoke`, `role:assign`, `member:add`, …); `detail` mang before/after (principal, role, scope, effect). Handler đã giữ `ctx`/`Auth(p)` cho actor. Helper mỏng `sink.emit(AuditEvent::Mutation{...})` mỗi site giữ đồng nhất. (Handler, không phải repo, vì actor + HTTP context ở đó; repo-level/DB-trigger không thấy actor request cho DELETE.)

## 6. Retention

Background prune task xóa row `ts < now() - audit.retention_days` (default 90) theo interval (vd mỗi giờ): `DELETE FROM audit.audit_log WHERE ts < now() - ($1 || ' days')::interval`. Best-effort; log số đã xóa. Giữ bảng full-volume có bound không cần partition (MVP; partition sau nếu cần).

## 7. Query API (no UI)

`GET /api/control-plane/audit` — **platform-admin only** (gate qua check admin-graph/`role:read` như access console; xác nhận gate chính xác lúc plan). Params: `actor` (uuid), `action` (string), `kind` (decision|mutation), `decision` (allow|deny), `from`/`to` (timestamptz), `workspace` (uuid), `limit` (default 100, max 500), `cursor` (keyset `(ts, id)` cho paginate ổn định). Trả newest-first `AuditEntryDto[]` + `next_cursor`. utoipa + openapi + client regen (dù chưa có trang, client vẫn current). Không endpoint ghi (append-only; writer + prune là mutator duy nhất).

## 8. Config

`audit.enabled` (default true — false bỏ wrap PDP + emit, cho perf/test), `audit.retention_days` (90), `audit.channel_capacity` (8192), `audit.batch_size` (256), `audit.flush_interval_ms` (1000). Default hợp lý để deploy stock audit ngay không cần tune.

## 9. Bảo mật

- **Append-only + admin-only read:** không API update/delete row; query API platform-admin-gated; prune là deleter duy nhất (retention).
- **Actor integrity:** actor từ principal đã xác thực (decision: `subject.user_id`; mutation: `ctx.principal`) — không phải request field. System event dùng `NULL` + `detail.source="system"`.
- **Best-effort là tradeoff tường minh:** overload thì sink drop event (log drop-count) — chấp nhận theo user; không bao giờ block/fail request, không làm yếu enforcement (audit là quan sát, trực giao với PDP decision).
- **Không lộ secret:** `detail` mang grant metadata (principal/role/scope/effect) + decision context (action/resource/reasons) — không token/password/query data.
- **PDP parity không đụng:** `AuditingPdp` delegate decision nguyên văn cho `CedarPdp`, chỉ quan sát — decision matrix không đổi.

## 10. Testing

- **Decision capture:** `AuditingPdp` quanh fake/real Pdp emit đúng 1 `Decision` event/`authorize` (cả allow + deny), đúng actor/action/resource/allow/reasons.
- **Mutation capture:** grant `assign`/`grant_put`/`grant_delete` qua handler emit `Mutation` event có actor + before/after.
- **Sink/writer:** event đẩy vào channel được batch-insert vào `audit.audit_log`; channel đầy drop không block (assert `emit` return ngay + drop counter tăng).
- **Query API:** filter (actor/action/kind/decision/from-to/workspace) + keyset paginate + newest-first; gate platform-admin (non-admin → 403).
- **Retention:** prune xóa row cũ hơn cutoff, giữ row mới.
- **PDP parity:** matrix regression B1a + object-scope suites pass không đổi (decorator không đổi decision).
- **Full gate:** BE `cargo test --workspace`, FE `lint/typecheck/test/build` (client regen cho audit DTO), openapi current, adversarial security review.

## 11. Câu hỏi mở (cho plan)

- **Vị trí crate:** `AuditEvent`/`AuditSink` ở crate `egret-audit` mới vs module trong `egret-core`. Mặc định: crate `egret-audit` nhỏ (egret-core giữ nhẹ dep; egret-auth + handler + egret-bin dep nó). Xác nhận lúc plan.
- **Helper `Resource`:** `AuditingPdp` cần `resource.workspace_id()/type_str()/id_str()` trên `egret_core::authz::Resource` — thêm (nhỏ, pure). Xác nhận chưa có.
- **Admin gate query API:** reuse gate access console dùng (`require(&pdp, &p, "role:read")` hoặc `audit:read` riêng). Mặc định: reuse `role:read` (platform admin đã có); `audit:read` riêng sạch hơn nhưng thêm catalog entry — quyết lúc plan.
- **Số migration:** kế tiếp sau 0075; touch `egret-metastore/src/lib.rs`.

---

*Design v1.0 (2026-07-11). Audit trail thật cho Effort B: `AuditingPdp` decorator bắt mọi allow/deny ở chokepoint PDP (zero PEP churn) + grant-mutation handler emit change event; cả hai vào một async best-effort sink batch-insert vào `audit.audit_log` append-only, kèm retention prune + query API platform-admin. Full-volume, best-effort, backend-only.*
