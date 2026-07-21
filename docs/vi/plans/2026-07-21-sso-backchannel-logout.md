# SSO Back-Channel Logout — Kế hoạch (tóm tắt)

> **Bản chi tiết (chuẩn, code từng bước):** `docs/en/plans/2026-07-21-sso-backchannel-logout.md`
> **Spec:** `docs/vi/specs/2026-07-21-sso-backchannel-logout-design.md`
> **Thực thi:** subagent-driven-development.

**Mục tiêu:** Endpoint `POST /api/auth/oidc/backchannel-logout` do Keycloak gọi, validate logout token và revoke session egret của user (theo `sub`).

## Task
1. **`verify_logout_token(jwks, iss, aud, token) -> Result<String>`** (thuần, trong `oidc.rs`): verify JWS RS256 với JWKS (theo `kid`), enforce `iss`/`aud`/`exp`, bắt buộc `events` chứa key back-channel-logout, **từ chối nếu có `nonce`** (đó là ID token), trả `sub`. Dùng `jsonwebtoken` (đã là dep qua machine-idp; xây key test kiểu machine-idp). Unit test: nhận token hợp lệ + loại 6 mode (thiếu events, có nonce, sai aud, sai iss, hết hạn, sai chữ ký).
2. **Handler `backchannel_logout`** (`oidc_routes.rs`, mount vào router `/api/auth/oidc` → `/api/auth/oidc/backchannel-logout`): form `logout_token`; CSRF-exempt (POST server-to-server, không cookie — xác nhận CSRF layer không chặn request không-cookie); `provider_jwks()` từ OidcService → `verify_logout_token` → `find_by_external_subject(sub)` → `revoke_all_for_user`; unknown sub → 200 no-op; `Cache-Control: no-store`; token sai → 400. Thêm `OidcService::{issuer, client_id, provider_jwks}` (JWKS: tái dùng metadata đã discover, hoặc fetch `jwks_uri` parse thành `jsonwebtoken::jwk::JwkSet`). Integration test: user có session → POST logout token hợp lệ → session `validate` = None.
3. **Config realm Keycloak + e2e best-effort:** thêm `attributes` client `egret` (`"backchannel.logout.url"` = URL egret in-cluster, `"backchannel.logout.session.required": "false"`) vào cả 2 realm JSON. e2e: login OIDC user → Keycloak admin API force-logout → assert `/api/auth/me` = 401 (nếu brittle → `test.fixme` + manual verify; integration test T2 là coverage chính). Recipe `just e2e-kind-backchannel-logout`.

## Verify cuối
fmt + clippy(`-D warnings`) + `cargo test -p egret-auth` (unit logout-token + integration revoke) + (best-effort) rebuild+realm-reimport+`just e2e-kind-backchannel-logout` hoặc manual verify.
