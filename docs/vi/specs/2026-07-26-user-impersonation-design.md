# User impersonation (đóng vai người dùng) — Thiết kế

> Bản tiếng Việt tham chiếu. Bản chuẩn:
> [`docs/en/specs/2026-07-26-user-impersonation-design.md`](../../en/specs/2026-07-26-user-impersonation-design.md)

## 1. Mục tiêu

Cho admin **đóng vai một user khác** để tái hiện lỗi user đó báo — thấy đúng thứ
họ thấy, gặp đúng quyết định phân quyền, và chạy đúng truy vấn dưới đúng
masking/row-filter Ranger của họ.

- **platform_admin** đóng vai bất kỳ user không phải admin.
- **ws_admin** đóng vai một thành viên thường của workspace mình quản, và chỉ
  trong workspace đó.

Mục đích: hỗ trợ/debug ("em không thấy bảng này", "trang này 403 với em"), không
phải làm việc thay user.

## 2. Ý tưởng lõi

Mọi engine token, catalog token, và quyết định phân quyền trong egret đều dựng từ
`Principal` của request — machine-IdP mint engine JWT per-user từ danh tính
principal, Cedar lấy principal làm subject, tầng workspace/RBAC đọc role của nó.
Nên impersonate chỉ cần đúng một việc:

> **Làm session resolve ra `Principal` của user đích, kèm một lớp phủ ghi nhớ
> admin thật.**

Data plane, Cedar, RBAC, UI đều tự bám theo danh tính đích **miễn phí** — không
đổi `egret-machine-idp`, `egret-query`, hay các engine token minter. Admin thật
được mang kèm để audit, hiện banner, và kiểm tra khoá-ws cho ws_admin.

## 3. Cơ chế: lớp phủ, không tráo session

Hai phương án đã cân nhắc:

- **Lớp phủ (chọn).** Một row gắn với session sẵn có của admin đánh dấu nó "đang
  đóng vai target T thay cho admin A". `SessionService` áp dụng khi validate.
  Cookie admin không đổi; session thật của admin không bị phá.
- Tráo session (loại). Mint token phiên impersonate riêng, đổi cookie, khôi phục
  token admin lúc thoát. Nhiều bộ phận động hơn (đổi cookie, khôi phục token — mà
  sessions chỉ lưu *hash* token nên không phục hồi được token admin), không lợi.

Lớp phủ đơn giản và an toàn hơn: danh tính/session thật của admin không bao giờ bị
tráo hay phá; impersonate là lớp tường minh, thu hồi được, có time-box.

### 3.1 Mô hình dữ liệu — migration `0095_impersonations.sql`

```sql
-- Lớp phủ impersonation đang hoạt động trên session của admin. Vắng = session là
-- admin đóng vai chính mình. Tối đa một row active mỗi session admin.
CREATE TABLE impersonations (
    id             UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    admin_session_id UUID      NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    impersonator_id  UUID      NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    target_user_id   UUID      NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    -- NULL = không giới hạn (platform_admin). Khác NULL = ws_admin, khoá trong
    -- đúng workspace này.
    workspace_scope_id UUID    REFERENCES workspaces(id) ON DELETE CASCADE,
    reason         TEXT        NOT NULL,
    started_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at     TIMESTAMPTZ NOT NULL
);

CREATE UNIQUE INDEX impersonations_one_per_session ON impersonations (admin_session_id);
```

`sessions` có cột `id` (từ migration đầu) để lớp phủ tham chiếu; `SessionService`
hiện resolve theo `token_hash`, nên sẽ đọc thêm `id` của row để tra lớp phủ.

## 4. Danh tính: `Principal` thêm impersonator

`egret-core/src/context.rs` — thêm lớp phủ tuỳ chọn vào `Principal`:

```rust
pub struct Impersonator {
    pub user_id: UserId,
    pub email: String,
    /// NULL cho platform_admin (không giới hạn); Some(ws) khoá một ws_admin.
    pub workspace_scope: Option<WorkspaceId>,
    pub expires_at: chrono::DateTime<chrono::Utc>,
}

pub struct Principal {
    // … các field sẵn có …
    /// Có iff request này là impersonation. user_id/email/role ở trên là của
    /// TARGET; đây là admin thật đứng sau.
    pub impersonator: Option<Impersonator>,
}
```

PDP subject bỏ qua `impersonator` — phân quyền quyết định theo target, đó chính là
điểm mấu chốt. `impersonator` chỉ được đọc bởi: tầng audit, endpoint `/me`
(banner), và kiểm tra khoá-ws cho ws_admin (§7).

## 5. Validate session áp dụng lớp phủ

`SessionService::validate(token)`:

1. Resolve row session (như hiện tại) → đây là danh tính **admin**, kèm `id` của
   session.
2. Tra row impersonation theo `admin_session_id = <id session>` chưa quá
   `expires_at`.
3. **Không có lớp phủ** → trả `Principal` admin, `impersonator: None` (giữ nguyên
   hành vi).
4. **Có lớp phủ active** → dựng `Principal` của **target** (role/permission/group
   của target — helper sẵn có, keyed theo `target_user_id`) và đặt
   `impersonator = Some(...)`.
5. **Lớp phủ hết hạn** → bỏ qua (trả principal admin) và best-effort xoá row. Đây
   là tự-về-admin: hết cửa sổ 30 phút thì request kế tiếp lại là admin. Session
   admin chưa bao giờ bị đụng nên không phải đăng nhập lại.

Impersonate lồng bất khả thi do cấu trúc: `validate` chỉ tra lớp phủ cho session
admin; request đang impersonate không có session riêng để gắn lớp phủ thứ hai, và
endpoint start (§6) từ chối khi caller đang impersonate.

## 6. Bắt đầu impersonate

Hai endpoint, chỉ khác cách xác định scope:

- `POST /api/admin/impersonate` — body `{target_user_id, reason, password?}`.
  Caller phải là `platform_admin`. `workspace_scope_id = NULL`.
- `POST /api/ws/{workspace}/impersonate` — body `{target_user_id, reason,
  password?}`. Caller phải giữ `ws_admin` trên `{workspace}`.
  `workspace_scope_id = <workspace đó>`.

Cả hai thực hiện, theo thứ tự:

1. **Từ chối nếu caller đang impersonate** (`principal.impersonator` có) — không
   lồng.
2. **Step-up re-auth** (§8). Sai → `401`, không ghi gì.
3. **Kiểm tra target:**
   - target ≠ caller (không tự đóng vai);
   - target không phải `platform_admin` → `403`;
   - với endpoint ws_admin: target là thành viên `{workspace}` và **không** phải
     `ws_admin` của nó → `403`.
4. Insert row impersonation (`expires_at = now() + 30 phút`), gắn với session
   admin hiện tại.
5. Emit audit `impersonation.start` (actor = impersonator, detail = target +
   workspace_scope + reason).

Trả `200` kèm trạng thái impersonation mới; client refetch `/me` và hiện banner.

## 7. Khoá workspace cho ws_admin

Khi `workspace_scope` là `Some(ws)`, principal bị đóng vai bị khoá trong đúng
workspace đó dù target thuộc nhiều ws:

- **`GET /api/me/workspaces`** chỉ trả `ws` (không phải toàn bộ membership của
  target).
- **`WsContext`** (`egret-workspace/src/context.rs`) từ chối mọi workspace khác
  `ws` với `403`, trước khi giải membership — nên Alice-as-Bob không vào được W2
  kể cả gõ URL, và không mint được engine token scope W2 (engine token chỉ mint
  bên trong một `WsContext` đã resolve).

platform_admin có `workspace_scope = None` → thấy toàn bộ membership của target,
không giới hạn.

Kiểm tra khoá nằm cạnh gate feature-entitlement đã thêm ở phần per-workspace
features — cả hai đều là "request này có được chạm workspace này không", giải một
lần trong `WsContext`.

## 8. Step-up re-authentication

Bắt đầu impersonate luôn cần bằng chứng danh tính mới + lý do:

- **Admin local** — nhập lại mật khẩu; verify bằng `verify_password` sẵn có so
  với `password_hash` của admin.
- **Admin OIDC** — re-auth qua IdP với `prompt=login` (tái dùng luồng OIDC sẵn
  có). Endpoint start trả `409` kèm URL authorize OIDC khi cần step-up; client
  redirect, khi quay lại impersonation được tạo với reason mang theo qua `state`
  của OIDC.

Reason bắt buộc (khác rỗng) cho cả hai, lưu vào row và audit start.

## 9. Đọc-ghi kèm quy trách nhiệm bằng audit

Impersonation là **đọc-ghi** — admin làm được mọi thứ target làm, kể cả mutation,
để tái hiện được lỗi đường ghi. Kiểm soát là **quy trách nhiệm, không chặn**:

- `AuditEvent::Mutation` thêm `impersonator: Option<Uuid>`. Mọi mutation có audit
  trong lúc impersonate ghi cả `actor` (target, danh tính chạy hành động) lẫn
  `impersonator` (admin thật). Helper audit đọc `principal.impersonator` — không
  sửa từng handler.
- Không hộp xác nhận từng thao tác (sẽ làm luồng nhiều bước không dùng nổi).
- Step-up lúc start (§8) là cổng; audit trail là trách nhiệm giải trình.

## 10. Thoát

- `POST /api/auth/impersonation/exit` xoá row lớp phủ của session caller và emit
  `impersonation.end`. Request kế tiếp lại là admin.
- `expires_at` 30 phút là chốt bị động (§5, bước 5): impersonation bị quên tự kết
  thúc.

## 11. `/me` và UI

`GET /api/auth/me` thêm:

```jsonc
"impersonating": {
  "target":       { "user_id": "…", "email": "…" },
  "impersonator": { "user_id": "…", "email": "…" },
  "workspace_scope": "…" | null,
  "expires_at":   "…"
} | null
```

- **Điểm vào.** Control Plane → Users → row action **Impersonate**
  (platform_admin). Workspace → Members → row action **Impersonate** (ws_admin;
  scope ngầm định theo nơi bấm). Dialog hiện target, ô **lý do** bắt buộc, và ô
  mật khẩu (local) hoặc nút **Xác thực lại bằng SSO** (OIDC).
- **Banner.** Thanh trên cùng nổi bật khi `impersonating` khác null: *"Đang đóng
  vai **{target}** · còn {đếm ngược} · [Thoát]"*. Không thể bỏ lỡ để admin không
  quên đang đóng vai người khác. Đọc từ `/me`; toast lúc start/exit (Toaster giờ
  đã mount).
- **Thoát** có ở banner và user menu.

## 12. Tóm tắt guard

| Guard | Ở đâu |
| --- | --- |
| Cấm đóng vai platform_admin | start endpoint, kiểm tra target |
| Cấm tự đóng vai | start endpoint |
| Không impersonate lồng | start endpoint + `validate` chỉ phủ session admin |
| ws_admin: target là thành viên thường của ws quản | ws_admin start endpoint |
| ws_admin khoá trong đúng một ws | `/me/workspaces` + `WsContext` |
| Step-up + lý do lúc start | start endpoint |
| 30 phút + thoát tay | `expires_at`, exit endpoint |
| Đọc-ghi, nhưng mọi mutation ghi cả hai bên | tầng audit |
| Thoát / hết hạn về admin | `validate` + exit endpoint |

## 13. Test

**Integration (sqlx):** platform_admin đóng vai user → `/me` báo `impersonating`,
`Principal` resolve là của target; ws_admin bị khoá (vào W2 → 403, `/me/workspaces`
chỉ có ws quản); cấm target platform_admin (403); cấm tự/lồng (403); step-up sai
mật khẩu (401); hết hạn → tự về admin; mutation lúc impersonate → audit có cả
impersonator; thoát xoá lớp phủ về admin.

**FE (vitest):** banner hiện từ `/me` khi impersonate và ẩn khi không; đếm ngược
đọc `expires_at`; Thoát gọi endpoint; dialog Impersonate bắt buộc lý do.

**E2e (Playwright):** platform_admin đóng vai một thành viên ws → banner hiện →
nav/quyền phản ánh target → Thoát về admin. Gate sau env var opt-in như các spec
e2e khác.

## 14. Ngoài phạm vi (có chủ đích)

- Thông báo cho user bị đóng vai (chốt: chỉ audit).
- Trang audit lọc theo impersonation (để sau).
- Impersonate theo lịch/uỷ quyền.
- Đóng vai cross-tenant ngoài ws mà ws_admin quản.
- Chế độ impersonate chỉ-đọc (thiết kế này là đọc-ghi kèm quy trách nhiệm).
