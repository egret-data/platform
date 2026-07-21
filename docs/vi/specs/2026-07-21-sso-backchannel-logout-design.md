# SSO Back-Channel Logout — Thiết kế

> **Trạng thái:** Đã duyệt thiết kế. Bước tiếp: viết plan.
> **Xây trên:** STS effort (#304–307). Đóng gap central-revocation đã defer ở SP1 (session egret độc lập khỏi Keycloak).
> *(Bản tiếng Anh là chuẩn: `docs/en/specs/2026-07-21-sso-backchannel-logout-design.md`.)*

**Mục tiêu:** Khi user bị logout / force-logout bên Keycloak, egret revoke session egret của user đó NGAY (thay vì đợi session tự hết hạn). Triển khai OIDC Back-Channel Logout 1.0: một endpoint egret do Keycloak gọi, nhận logout token đã ký và revoke.

## 1. Quyết định (đã chốt)
**Granularity = theo `sub`** — `sub` trong logout token → egret user, egret revoke **tất cả** session của user đó (`SessionService::revoke_all_for_user`). Không schema change (không cột `sid`, không capture lúc login); nhất quán với disable→revoke-all; đủ cho data portal (user thường 1 session). (Phương án per-`sid` bị loại vì phức tạp thừa.)

## 2. Seam sẵn có (tái dùng)
- `OidcService` (`crates/egret-auth/src/oidc.rs`) đã discover provider metadata + JWKS của Keycloak và validate ID token — cùng trust dùng để verify chữ ký/`iss` của logout token.
- `UserRepo::find_by_external_subject(sub) -> Option<User>` (`users.rs:89`) — map `sub`→egret user (đã dùng ở `provision_and_sync` lúc login).
- `SessionService::revoke_all_for_user(user_id)` (`session.rs`) — xóa các row `sessions`; request kế với cookie cũ fail `validate`.
- `OidcRoutesState` đã có `oidc`, `users`, `sessions` — handler cần đúng chừng đó, không thêm state.

## 3. Thành phần
### 3.1 `verify_logout_token(&OidcService, token) -> Result<String>` (trả `sub`)
Validator trong `crates/egret-auth/src/oidc_routes.rs` (hoặc `oidc.rs`), verify logout token Keycloak-ký theo OIDC Back-Channel Logout 1.0. Chỉ nhận nếu TẤT CẢ đúng, không thì lỗi:
- chữ ký verify với provider JWKS (RS256, theo `kid`);
- `iss` == Keycloak issuer cấu hình; `aud` chứa `client_id` của egret;
- có `iat`, `exp` còn hạn (leeway nhỏ);
- claim `events` là object chứa key `http://schemas.openid.net/event/backchannel-logout`;
- **KHÔNG có claim `nonce`** (phân biệt logout token với ID token);
- có `sub` → trả về.
Dùng logout-token support của crate `openidconnect` nếu có; không thì verify JWS thủ công với JWKS đã discover (`jsonwebtoken` đã là dependency qua egret-machine-idp).

### 3.2 Handler `POST /api/auth/backchannel-logout`
Mount cạnh OIDC callback (CSRF-exempt — Keycloak gọi server-to-server, body `application/x-www-form-urlencoded` `logout_token=<JWT>`, không cookie). Luồng:
```
logout_token → verify_logout_token → sub
  → UserRepo::find_by_external_subject(sub)
      → Some(user) → SessionService::revoke_all_for_user(user.id) → 200
      → None                                                       → 200 (no-op)
```
Set `Cache-Control: no-store` (theo RFC).

### 3.3 Config realm Keycloak
Thêm vào client `egret` trong `deploy/helm/egret/files/egret-realm.json` (và `deploy/keycloak/egret-realm.json`):
- `Backchannel Logout URL` = URL egret Keycloak tới được, vd `http://egret.egret-system.svc.cluster.local:8080/api/auth/backchannel-logout` (Service DNS in-cluster, cùng host `catalog.public_url`) hoặc `http://egret.localtest.me/api/auth/backchannel-logout`.
- `Backchannel Logout Session Required` = **false** (key theo `sub`, không `sid`).
Trong realm JSON là client `attributes`: `"backchannel.logout.url"` + `"backchannel.logout.session.required": "false"`. Ghi chú là bước admin cho deploy ngoài kind.

## 4. Xử lý lỗi
Theo OIDC Back-Channel Logout 1.0:
- Logout token sai (chữ ký/`iss`/`aud`/hết hạn/có `nonce`/thiếu `events`) → **400** OAuth-style JSON error; không revoke.
- `sub` lạ (user chưa từng login egret) → **200** no-op (không lộ user tồn tại).
- JWKS provider không tới được → **400** (không validate được → không revoke).
- `Cache-Control: no-store` mọi response.
- Revoke idempotent (logout token lặp chỉ re-revoke) — không cần track `jti`/replay.

## 5. Bảo mật
Endpoint không auth bằng cookie nhưng bảo vệ bằng chữ ký Keycloak — chỉ Keycloak mint được token qua `verify_logout_token`. CSRF-exempt vì là POST server-to-server không cookie (cùng posture kernel-token callback). Không trả data per-user nên đường unknown-`sub`→200 không lộ gì.

## 6. Kiểm thử
- **Unit (`verify_logout_token`):** ký token bằng test key + stub JWKS (như test verify machine-idp). Nhận token hợp lệ (có `events`, không `nonce`, đúng `iss`/`aud`). Loại từng mode: sai chữ ký, sai `aud`, sai `iss`, hết hạn, **có `nonce`**, **thiếu `events`**.
- **Integration (DB + handler):** seed user có session sống, `POST` logout token hợp lệ cho `sub` đó → assert session `validate` giờ trả `None`; `sub` lạ → `200`, không revoke, không lỗi.
- **Live e2e (best-effort, tùy chọn):** cấu hình backchannel URL realm, dùng Keycloak **admin API force-logout user** (bắn back-channel logout tới client) rồi assert session egret chết. Nếu brittle (realm config + reachability), fallback manual verify + dựa integration test (authoritative — chạy verify→sub→revoke đầy đủ).

## 7. Phân rã task (cho plan)
1. Validator `verify_logout_token` + unit test (nhận + 6 mode loại).
2. Handler `POST /api/auth/backchannel-logout` + mount route (CSRF-exempt) + integration test (revoke khi token hợp lệ; unknown-sub no-op).
3. Config realm Keycloak backchannel URL (cả 2 realm JSON) + best-effort live e2e / manual verify.

## 8. Ngoài phạm vi
- RP-initiated / front-channel logout (egret logout → end Keycloak session) — chiều ngược, feature riêng.
- Revoke per-`sid` (đã loại ở §1).
- Front-channel logout iframe.
