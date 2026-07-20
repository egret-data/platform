# Egret STS / Token Broker — Kế hoạch (tóm tắt)

> **Bản chi tiết (chuẩn, có code từng bước):** `docs/en/plans/2026-07-20-egret-sts-token-broker.md`
> **Spec:** `docs/vi/specs/2026-07-20-egret-sts-token-broker-design.md`
> **Thực thi:** subagent-driven-development (implementer + spec-review + code-quality-review mỗi task).

**Mục tiêu:** machine-IdP của egret thành nơi phát duy nhất engine token per-user (như catalog token đã làm), để mọi user đã login — local hay SSO — query được engine mà không cần Keycloak refresh token.

## Các task (thứ tự trust-trước, mint-sau)

1. **Group-names + `engine_claims` (thuần).** `GroupRepo::group_names(user_id)` (egret-auth/groups.rs) + hàm thuần `engine_claims(username, groups, issuer, iat, ttl)` → JSON `{iss, sub, preferred_username, groups, aud:"engine", iat, exp}` (egret-bin/engine_minter.rs mới). Unit test.

2. **`MachineIdpEngineMinter`.** Impl `EngineTokenMinter`: `ensure_username` + `group_names` → `engine_claims` → `sign_claims(active key)` → `EngineToken`. Không Keycloak, không refresh token. Test integration + **regression local-admin không refresh token vẫn mint được**.

3. **Engine trust về machine-IdP.** `main.rs:1514` → `engine_jwks_uri = {catalog.public_url}/.well-known/jwks.json`, `engine_oidc_issuer = catalog.public_url`, **vô điều kiện OIDC**. Audience thống nhất `"engine"` (StarRocks DDL engines.rs:2358 `"starrocks"`→`"engine"` + Trino jwt config). Test DDL template.

4. **Swap minter + xoá broker Keycloak.** `main.rs:1151` dựng `MachineIdpEngineMinter` (TTL 900s = độ mịn revoke); `git rm engine_broker.rs`. Build workspace + consumer test (kernel/notebook/BI) xanh (đi qua trait, không đổi).

5. **JIT ghi group lúc OIDC login.** `oidc_routes.rs` callback: create-or-link user + `replace_user_groups` từ group claim → `user_groups` (nguồn authz runtime). Ngừng dùng refresh token cho engine. Test callback persist group.

6. **Session egret độc lập.** `session.rs::resolve`: reject khi user disabled / idle > 8h / lifetime > 24h; bump `last_seen_at` (sliding). Migration thêm cột `last_seen_at`. Test disable + idle.

7. **Live e2e (kind).** `sts-broker-kind.spec.ts` + recipe `just e2e-kind-sts`: (a) **local admin → notebook `starrocks("SELECT 1")` → row** (không Keycloak/refresh token — bằng chứng đinh); (b) SSO parity admin full vs member masked; (c) deployment OIDC-off vẫn query được.

## Verify cuối
fmt + clippy(`-D warnings`) + `cargo test --workspace` + FE lint/tsc/build + `just e2e-kind-sts` xanh + log backend **không còn** `no stored refresh token for user`.

## Rollout (không phải task)
Engine cũ trust Keycloak: dev thì re-provision; prod thì dual-trust (`egret_machine` song song `egret_oidc`) rồi cắt về egret-only. Sub-project 2 (CRUD user local + UI) xây trên cái này.
