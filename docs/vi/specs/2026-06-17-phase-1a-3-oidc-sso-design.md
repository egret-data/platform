# Phase 1a-3 — OIDC / SSO — Spec thiết kế

> **Trạng thái:** Bản tham khảo tiếng Việt · **Ngày:** 2026-06-17 · **Phiên bản:** 0.1
> ⚠️ Bản gốc (canonical) là tiếng Anh: [`docs/en/specs/2026-06-17-phase-1a-3-oidc-sso-design.md`](../../en/specs/2026-06-17-phase-1a-3-oidc-sso-design.md). Khác biệt thì ưu tiên bản tiếng Anh.
> Tiếp nối [spec identity Phase 1a](2026-06-17-phase-1a-identity-design.md) §8 và [1a-1 AuthN](../plans/2026-06-17-phase-1a-1-authn-session.md) + [1a-2 Workspace & RBAC](../plans/2026-06-17-phase-1a-2-workspace-rbac.md) (group resolution đã wired).

---

## 1. Mục tiêu & phạm vi

Thêm login OIDC/SSO: authorization-code + PKCE với IdP ngoài, JIT provisioning, và **đồng bộ group từ IdP** khi login (role chảy qua cơ chế group→role của 1a-2). Chỉ backend/API.

**Trong phạm vi**
- `OidcService` (trên crate `openidconnect`): discovery, authorize-URL kèm PKCE + nonce + state, exchange code, validate id_token.
- Routes `/api/auth/oidc/login` + `/api/auth/oidc/callback`; PKCE/nonce/state tạm trong cookie mã hoá ngắn hạn.
- JIT: match theo `sub`, upsert user, cập nhật profile; **sync group (replace, chỉ oidc-source)** vào `user_groups`.
- Test IdP thật bằng **Keycloak** (testcontainers, gated) + Keycloak trong compose dev.

**Ngoài phạm vi (hoãn)**
- Account linking theo email (bảo mật: không auto-link; xem §10) → tương lai.
- Map claim→role trực tiếp qua `idp_role_mapping` (hàm `map_idp_claims_to_roles` từ 1a-2 chưa dùng) → tương lai.
- IdP mapping mức workspace; nhiều IdP cùng lúc → tương lai.
- Frontend nút login → Phase 0b.

## 2. Quyết định (validate với Metabase & Superset)

| Quyết định | Lựa chọn | Bằng chứng |
|---|---|---|
| Thư viện OIDC | **crate `openidconnect`** | Tránh tự viết validate chữ ký/JWKS/nonce. |
| Cơ chế IdP→role | **Sync group → group→role** (idp_role_mapping hoãn) | Superset `AUTH_ROLES_MAPPING` + `AUTH_ROLES_SYNC_AT_LOGIN`; Metabase `sso/` `group-membership`/`sync-group`. |
| Chính sách sync group | **Replace (revoke khi rời)**, chỉ membership oidc-source | Superset/Metabase sync (replace) group membership mỗi login. |
| State tạm | **Cookie mã hoá ngắn hạn** (PrivateCookieJar) | Stateless, đa-instance; bảo vệ tính bí mật + toàn vẹn PKCE verifier. |
| Match account | **Theo `sub`**; email trùng → `Conflict` (không auto-link) | Chống takeover qua email IdP chưa verify. |
| Test OIDC | **Keycloak thật (testcontainers, gated) — cả 2 tầng** | Discovery/JWKS/token thật; ROPC (validate nhanh) + full auth-code+PKCE+nonce. |

## 3. Kiến trúc

OIDC nằm trong `egret-auth`. Thành phần mới/đổi:

```mermaid
sequenceDiagram
  participant B as Browser
  participant A as egret-auth (oidc routes)
  participant K as IdP (Keycloak)
  B->>A: GET /api/auth/oidc/login
  A->>A: OidcService.start() → url+state+nonce+pkce
  A->>B: 302 to IdP; Set-Cookie egret_oidc_state (encrypted, 10m)
  B->>K: login form
  K->>B: 302 /callback?code&state
  B->>A: GET /api/auth/oidc/callback?code&state (+ state cookie)
  A->>A: verify state==cookie.state
  A->>K: OidcService.finish(code, pkce, nonce) → validate id_token → claims
  A->>A: provision_and_sync(claims): upsert user (by sub), sync oidc groups
  A->>A: SessionService.create → set session+csrf cookies; clear state cookie
  A->>B: 302 post_login_redirect
```

- `egret-auth/src/oidc.rs` — `OidcService`, `OidcClaims`, `OidcStart`, `provision_and_sync`.
- `egret-auth/src/groups.rs` — `GroupRepo` (upsert oidc group, sync user oidc-group membership).
- `egret-auth/src/users.rs` — thêm `find_by_external_subject`, `insert_oidc`, `update_profile`.
- `egret-auth/src/routes.rs` — handler `oidc_login`, `oidc_callback`; `AuthState` thêm `oidc: Option<OidcService>` + `cookie_key: cookie::Key`.

## 4. `OidcService` (interface)

```rust
pub struct OidcClaims {
    pub subject: String,
    pub email: String,
    pub name: String,
    pub groups: Vec<String>,
}

pub struct OidcStart {
    pub authorize_url: String,
    pub state: String,
    pub nonce: String,
    pub pkce_verifier: String,
}

pub struct OidcService { /* openidconnect CoreClient + http client + groups_claim */ }

impl OidcService {
    pub async fn discover(cfg: &egret_config::OidcConfig) -> Result<Self>;
    pub fn start(&self) -> OidcStart;
    /// Exchange code (kèm PKCE verifier) + validate id_token (chữ ký qua JWKS, iss,
    /// aud=client_id, exp, nonce). Trích claims kèm groups claim.
    pub async fn finish(&self, code: &str, pkce_verifier: &str, nonce: &str) -> Result<OidcClaims>;
}
```
> `groups` đọc từ `groups_claim` cấu hình (mặc định `"groups"`); nhận mảng JSON string.

## 5. Cookie state tạm

- `egret_oidc_state` — cookie **mã hoá** (`axum_extra::extract::cookie::PrivateCookieJar`, key = `cookie::Key` từ `[security].secret_key`).
- Value: JSON `{ state, pkce_verifier, nonce }`. Thuộc tính: `HttpOnly`, `SameSite=Lax`, `Secure` (theo `session.secure`), `Max-Age=600` (10 phút), `Path=/api/auth/oidc`.
- Khi callback: giải mã, assert `query.state == cookie.state`, rồi xoá cookie. Cookie thiếu/hết hạn/lệch → `InvalidInput` (400).

## 6. Routes

| Method | Path | Hành vi |
|---|---|---|
| GET | `/api/auth/oidc/login` | Nếu `oidc` tắt → 404. Ngược lại `OidcService.start()`, set cookie state mã hoá, 302 tới `authorize_url`. |
| GET | `/api/auth/oidc/callback?code&state` | Verify state vs cookie → `finish` → `provision_and_sync` → `SessionService.create` → set session+csrf cookie, xoá state cookie → 302 tới `post_login_redirect`. |

Cả hai route miễn CSRF (chưa có session app; OAuth `state` là phòng vệ CSRF cho flow này), mount trên sub-router public (không `csrf_layer`).

## 7. JIT provisioning + sync group

`provision_and_sync(users, groups, claims) -> Result<UserId>`:

1. **Match theo `sub`:** `users.find_by_external_subject(&claims.subject)`.
   - Có → `users.update_profile(id, email, name)`, `touch_last_login`.
   - Không → `users.insert_oidc(sub, email, name)`. Nếu `email` unique violation (email đã thuộc account khác) → `Error::Conflict` (không auto-link).
2. **Sync group (replace, chỉ oidc-source):**
   - Mỗi name trong `claims.groups`: `groups.upsert_oidc_group(name)` (insert `source='oidc'` on-conflict-do-nothing; trả id).
   - `groups.sync_user_oidc_groups(user_id, &claims.groups)`: trong 1 transaction — `DELETE FROM user_groups WHERE user_id=$1 AND group_id IN (SELECT id FROM groups WHERE source='oidc')`, rồi insert claim set. **Membership manual (`source='local'`) giữ nguyên.**
3. Trả `user_id`. Role resolve qua group→role có sẵn (1a-2 `Authorizer` + nhánh group của `SessionService.platform_roles`) — không ghi role trực tiếp ở đây.

`GroupRepo` (mới, `egret-auth/src/groups.rs`):
```rust
pub struct GroupRepo { db: Db }
impl GroupRepo {
    pub fn new(db: Db) -> Self;
    pub async fn upsert_oidc_group(&self, name: &str) -> Result<uuid::Uuid>;
    pub async fn sync_user_oidc_groups(&self, user_id: UserId, names: &[String]) -> Result<()>;
}
```

## 8. Data model

Không bảng mới. Một migration:

**`migrations/0005_oidc.sql`**
```sql
CREATE UNIQUE INDEX users_oidc_subject_uniq
    ON users (external_subject)
    WHERE provider = 'oidc' AND external_subject IS NOT NULL;
```
(`groups.source`, `users.external_subject`, `user_groups` đã có từ `0002`.)

## 9. Bổ sung cấu hình

```toml
[oidc]                     # OIDC chỉ hoạt động khi enabled = true
enabled = false
issuer_url = ""
client_id = ""
client_secret = ""
redirect_url = "http://localhost:8080/api/auth/oidc/callback"
scopes = ["openid", "email", "profile"]   # groups lấy qua group mapper của IdP, không qua scope
groups_claim = "groups"
post_login_redirect = "/"

[security]
# key base64 ≥64 byte để mã hoá cookie state OIDC (PrivateCookieJar).
secret_key = ""
```
- Nếu `oidc.enabled = true` mà `security.secret_key` rỗng/ngắn → **fail khi boot** kèm thông báo rõ (gợi ý cách sinh key).
- `egret-config` expose struct `OidcConfig` và `SecurityConfig`.

## 10. Bảo mật

- **Validate id_token** (trong `finish`, qua `openidconnect`): chữ ký theo JWKS IdP, `iss`==issuer, `aud`==client_id, `exp`, và **nonce** khớp giá trị trong cookie state.
- **OAuth state**: `state` callback phải bằng `state` cookie (CSRF cho redirect).
- **PKCE**: verifier chỉ trong cookie mã hoá; gửi trong token exchange; không lộ.
- **Không auto-link**: chỉ match theo `sub`; email đã thuộc account khác → `Conflict`, không link ngầm (chống takeover qua email IdP chưa verify).
- **Secret**: `client_secret` và `security.secret_key` không bao giờ log; `secret_key` bắt buộc khi bật OIDC.
- **Sync group** chỉ ghi lại membership `source='oidc'` — không thể leo thang qua, hay xoá, group/role cấp thủ công.
- Cookie state scope `/api/auth/oidc`, ngắn hạn (10 phút).

## 11. Test

**Tầng 0 — nhanh, không IdP:**
- Unit: hình dạng logic group-replace của `provision_and_sync`; validate config (secret_key bắt buộc khi enabled).
- DB-gated: `provision_and_sync` với `OidcClaims` tự dựng — `sub` mới tạo oidc user; `sub` cũ update profile + giữ id; email trùng local user → `Conflict`; sync group thay group oidc nhưng group thêm thủ công (`source='local'`) còn nguyên; `GroupRepo` upsert/sync.

**Tầng 1 — Keycloak thật (testcontainers, gated), cả 2 sub-tầng:**
- Realm pre-provisioned (`deploy/keycloak/egret-realm.json`): client confidential `egret` (secret, redirect URI, bật direct-access-grant, PKCE), user `tester` (password, email) trong group `data-team`, và **groups protocol mapper** để id_token mang `groups`.
- Test spin Keycloak qua testcontainers (import realm), rồi:
  - **(a) ROPC validate:** lấy id_token thật qua direct-access-grant; assert `OidcService` validate (chữ ký JWKS, iss, aud) và trích `sub`/`email`/`groups`.
  - **(b) Full auth-code+PKCE+nonce:** `OidcService.start()` → drive form login Keycloak headless (GET trang login, trích form action + cookie auth-session, POST credential `tester`, bắt `code` từ redirect `Location`) → `OidcService.finish(code, pkce_verifier, nonce)` validate id_token thật end-to-end → assert claims.
- **Gating:** chỉ chạy khi có Docker/Keycloak (testcontainers tự phát hiện Docker; ngược lại self-skip như DB test), giữ suite offline-green. CI chạy.

**Dev stack:** thêm service `keycloak` vào `deploy/docker-compose.dev.yml` (import cùng realm) để login OIDC local/manual với IdP thật; ghi rõ giá trị `[oidc]` khớp realm.

## 12. Phân rã cho plan

Task gợi ý: (1) migration `0005` + `egret-config` `OidcConfig`/`SecurityConfig` (+ validate boot); (2) method oidc cho `UserRepo` + `GroupRepo` + DB test; (3) `provision_and_sync` + DB test (Tầng 0); (4) `OidcService` trên `openidconnect` (discover/start/finish); (5) helper cookie state mã hoá; (6) route `oidc_login`/`oidc_callback` + `AuthState`/wiring; (7) realm json Keycloak + service compose; (8) harness Keycloak testcontainers + test ROPC validate; (9) test full auth-code form-login; (10) verify cuối + e2e.

## 13. Tham khảo

- Superset: `superset/superset/security/manager.py` (`sync_role_definitions`, oauth user info), `config.py` (`AUTH_ROLES_MAPPING`, `AUTH_ROLES_SYNC_AT_LOGIN`).
- Metabase: `metabase/src/metabase/sso/` (`oidc`, `common.clj`; ngữ nghĩa `group-membership`/`sync-group`).
- Egret: [spec identity](2026-06-17-phase-1a-identity-design.md) §8; [plan 1a-2](../plans/2026-06-17-phase-1a-2-workspace-rbac.md) (group resolution + `map_idp_claims_to_roles`).
