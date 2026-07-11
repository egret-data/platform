# B3a — BI data-plane per-user delegation

> **Bản tham chiếu tiếng Việt.** Bản chuẩn là [tiếng Anh](../../en/specs/2026-07-11-b3a-bi-data-plane-per-user-delegation-design.md) — khác biệt lấy bản EN làm gốc.
>
> **Trạng thái:** Design (đã duyệt để lên plan) · **Ngày:** 2026-07-11 · **Phiên bản:** 1.0
>
> **Xây trên:** F2 OIDC delegation (per-user engine JWT cho notebook plane — DONE, live-verified PRs #99–104), B2-1b/B2-2 (object-scoped BI data endpoints).
>
> Increment B3 đầu tiên. Đưa per-user engine delegation của F2 sang **BI data plane** (SQL Lab, saved chart/dashboard/dataset data). Hiện `run_query` chạy trên workspace connection shared service creds; B3a cho nó present per-user JWT của NGƯỜI GỌI → engine authorize per-user. Ranger federation là increment riêng (B3b).

---

## 1. Mục tiêu & scope

F2 đã giao cơ chế per-user OIDC/Keycloak delegation (JWT engine short-lived downscoped = user, mint bởi broker `EngineTokenMinter`; connector đã nhận `bearer_token`) — và proven live cho **notebook/kernel** plane. **BI plane** (`egret_query::execution::run_query`, gọi bởi SQL Lab submit + object-scoped chart/dashboard/dataset data endpoint) vẫn truyền `bearer_token = None` → BI query chạy như **shared service user** của workspace connection — đường data non-delegated cuối cùng.

B3a đóng gap bằng cách tái dụng máy F2: trên connection đánh dấu **delegated**, đường BI query mint JWT caller + present làm connector bearer → engine (và authz nó chạy — native ACL, Keycloak-group RBAC, hay Ranger) enforce **per-user**.

**Trong scope:** flag `delegated` per-connection trên `databases` (+ input/DTO/UI); tham số `bearer_token` cho `run_query`; helper `resolve_delegation` trong egret-bi áp ở mọi BI `run_query` call site (SQL Lab submit, `POST /charts/{id}/data`, `POST /dashboards/{id}/cards/{card_id}/data`, `POST /datasets/{id}/data`); inject `EngineTokenMinter` Arc sẵn có vào `BiState`; đường lỗi governed-sharing (viewer không có engine data access → 403 thân thiện).

**Ngoài scope:** Ranger grant-federation — project `access_grants` → engine authz (B3b); vended per-user S3 creds (F2-P6); notebook plane (đã delegated qua F2); non-delegated connection (không đổi).

**Quyết định chốt khi brainstorm:** làm B3a trước, rồi B3b (Ranger) chu kỳ riêng; **mọi** BI data (SQL Lab + saved shared object) chạy theo danh tính **người gọi/viewer** (per-user governed — data dashboard được share vẫn bị engine data access của viewer gác; object-share chỉ cho thấy định nghĩa); delegation opt-in **per connection** (flag `delegated`); non-OIDC user đụng delegated connection bị **deny (403)**, không fallback ngầm (không governance bypass); flag là **cột riêng**; JWT **mint per-request** (broker cache nội bộ — chưa thêm cache tầng BI).

## 2. Data model — flag `delegated`

`ALTER TABLE databases ADD COLUMN delegated boolean NOT NULL DEFAULT false;` (migration mới; repo đã qua 0074). Ngữ nghĩa:
- **`delegated = false` (default):** behavior cũ — connector dùng service creds (`secret`) + `session_user` hiện tại. Không per-user JWT. Non-breaking mọi connection sẵn có.
- **`delegated = true`:** BI query chạy per-user JWT của caller. Admin đánh dấu delegated chỉ khi engine đích đã cấu hình nhận Keycloak JWT (config Trino/StarRocks JWT của F2). Connector present bearer (ưu tiên hơn basic-auth, per F2-P3) = danh tính caller.

Expose `delegated` trên `DatabaseInput` (create/update), `DatabaseDetailDto`/`DatabaseListDto`, + toggle ở form connection DatabasesPage. Default false ở mọi DTO nên bỏ trống = giữ behavior.

## 3. Execution path

### 3.1 `run_query` thêm bearer
`run_query(..., session_user, limits, cancel)` hiện gọi `factory.build(&db, secret, session_user, None)`. Thêm tham số `bearer_token: Option<String>` thread vào `factory.build(&db, secret, session_user, bearer_token)`. `ConnectorFactory::build` đã nhận `bearer_token` (F2), `TrinoConnector`/`StarRocksConnector` đã dùng (bearer ưu tiên basic-auth, mọi request site). Không đổi connector.

### 3.2 Helper `resolve_delegation` (egret-bi)
Một helper DRY logic per-call-site:

```
async fn resolve_delegation(st, ctx, database_id)
    -> Result<(Option<String> /*session_user*/, Option<String> /*bearer*/), BiApiError>
```

- Load database. Nếu `!db.delegated`: return `(session_user hiện tại, None)` — behavior cũ (session_user derive như hôm nay cho audit/X-Trino-User).
- Nếu `db.delegated`:
  1. Mint JWT caller: `st.engine_minter.mint(ctx.principal.user_id.0)`.
     - `Err(NoRefreshToken)` → `Error::Forbidden` (delegated connection cần OIDC session; user non-OIDC/password không query được). Message rõ ("connection này cần đăng nhập SSO").
     - `Err(other)` → `Error::Internal`.
  2. Derive `session_user` = username caller qua `UserRepo::ensure_username(user_id)` — PHẢI khớp JWT `preferred_username` (**gotcha F2 live**: engine match login/session user với JWT principal; lệch bị reject "login name X not matched to user Y"). Keycloak `preferred_username` = username egret, nên `ensure_username` là nguồn cho cả hai.
  3. Return `(Some(username), Some(jwt.access_token))`.

Mọi BI `run_query` call site gọi `resolve_delegation` trước rồi truyền `(session_user, bearer)` vào `run_query`.

### 3.3 Call sites
Áp ở mọi BI data path (đều qua `run_query`):
- SQL Lab submit (interactive).
- `POST /ws/{ws}/charts/{id}/data` (B2-1b) — chạy spec chart đã lưu theo **caller** (viewer).
- `POST /ws/{ws}/dashboards/{id}/cards/{card_id}/data` (B2-1b) — chạy chart của card theo **caller** (transitive trên dashboard grant, nhưng DATA chạy theo engine identity của viewer).
- `POST /ws/{ws}/datasets/{id}/data` (B2-2) — dataset preview theo caller.

## 4. Governed sharing

Theo quyết định per-user, data saved object được share chạy theo engine identity của **viewer**. Object-share (B2) cho thấy *định nghĩa*; *data* vẫn bị per-user authz của engine gác. Hệ quả: viewer có object grant nhưng không có engine access tới table → engine deny. Surface lỗi thân thiện (map connector auth/permission error → 403 "bạn không có data access tới nguồn này") thay vì 500 thô, để UI giải thích. Đây là mô hình governed đã chốt, không phải regression.

## 5. State wiring

`BiState` thêm `engine_minter: Arc<dyn EngineTokenMinter>` (seam F2, đã ở `egret-analytics-api`/`egret-notebook`) + truy cập `UserRepo` cho `ensure_username` (BiState đã có `sessions`/`db`; thêm `users: UserRepo` hoặc reuse handle sẵn). Ở `egret-bin/main.rs`, `engine_minter` Arc build (~dòng 1079) SAU `BiState` (~dòng 1060); relocate minter construct lên trên `BiState` (như B1b-3 relocate block pdp/bus) + truyền `engine_minter.clone()` vào `BiState`, reuse CÙNG Arc chia sẻ với `NotebookState`/`KernelState` (một broker).

## 6. Bảo mật

- **Delegation, không impersonation:** JWT bearer = caller (short-lived, downscoped, `aud`=engines). egret không giữ act-as credential đặc quyền trên đường BI; engine authorize user thật. (F2-proven: Trino enforce per-user JWT qua TLS; StarRocks qua patched driver.)
- **Không bypass:** non-OIDC user trên delegated connection bị deny (403) → không lách per-user enforce bằng password login. Non-delegated connection là mô hình shared-service tường minh (admin chọn).
- **session_user ↔ JWT binding:** `session_user` derive từ `ensure_username` = JWT `preferred_username`; lệch bị engine reject (defense-in-depth; helper set cả hai từ một nguồn).
- **Governed viewer:** viewer của shared-object chạy theo chính mình → engine gác data. Object-share không bao giờ nới rộng data-plane access.
- **Token:** JWT mint per-request qua broker (cache/refresh nội bộ) + pass làm bearer; không persist ở tầng BI, redacted trong log (Debug redaction của `EngineToken` — F2).

## 7. Testing

- **Delegated database:** với mock `EngineTokenMinter`, `run_query` nhận bearer mint được + `session_user`=username; minter trả `NoRefreshToken` → endpoint 403; delegated chart/dashboard/dataset data endpoint thread bearer.
- **Non-delegated database:** `run_query` nhận `bearer=None` + `session_user` hiện tại — behavior y hệt hôm nay (regression guard).
- **Cả 4 call site** qua `resolve_delegation` (assert mỗi cái mint khi delegated, None khi không).
- **Reuse connector:** test bearer `connector_trino`/`connector_starrocks` sẵn có phủ việc bearer tới wire; không cần test connector mới.
- **Full gate:** BE `cargo test --workspace`, FE `lint/typecheck/test/build`, openapi + client current (field `delegated` mới), adversarial security review.

## 8. Câu hỏi mở (cho plan)

- **session_user hôm nay:** xác nhận BI `run_query` call site hiện truyền gì làm `session_user` (username caller cho X-Trino-User audit, hay service user). Helper giữ nguyên cho non-delegated; delegated ép `ensure_username`. Inspect giá trị hiện tại lúc plan.
- **Độ mịn map lỗi:** connector trả `EngineError`; plan phải map auth/permission failure trên delegated connection → 403 với message data-access, khác 500 chung. Xác nhận connector surface được auth error phân biệt, hoặc wrap thận trọng (delegated-connection engine error giống auth → 403).
- **Số migration:** repo đã qua 0074; plan chọn số kế tiếp + touch `egret-metastore/src/lib.rs` (sqlx re-embed).

---

*Design v1.0 (2026-07-11). Đưa per-user engine JWT delegation của F2 sang BI data plane: flag `delegated` per-connection cho `run_query` present JWT engine mint được của caller (reuse broker F2 + connector bearer), nên BI query — SQL Lab lẫn shared saved object — chạy theo viewer dưới per-user authz của engine. Ranger grant-federation là B3b.*
