# Notebook F2 — Ủy quyền OIDC hợp nhất (thiết kế · bản tham chiếu)

> Bản tiếng Anh là chuẩn: `docs/en/specs/2026-06-30-notebook-f2-oidc-delegation-design.md`. File này là bản tham chiếu rút gọn.
> Trạng thái: nháp để review · 2026-06-30 · phụ thuộc F1, D1a.

## Mục tiêu
Kernel Daft đọc **Iceberg/Trino/StarRocks với tư cách end-user**, governed, **không giữ credential đặc quyền** trong kernel (kernel chạy code tuỳ ý của user). Thay 2 stopgap của F1 bằng **một JWT OIDC** từ **Keycloak** cho tất cả:
- Trino **impersonation** (principal đặc quyền set `X-Trino-User`) → **JWT OIDC per-user**.
- **Opaque catalog token** per-user (mint/revoke) → **cùng JWT đó**, catalog tự validate.

**Vì sao không impersonate trong kernel:** credential "act-as" đặc quyền nằm trong process kernel → user đọc ra rồi giả mạo bất kỳ ai (leo quyền). Lời giải = **delegation, không phải impersonation**: kernel chỉ giữ token ngắn hạn *chính là user*, giới hạn đúng quyền user.

## Phạm vi (F2)
1. **Keycloak realm**: 1 issuer, group claims, client/audience cho egret + catalog + Trino + StarRocks; seed user/group dev.
2. **egret token broker**: đổi session user → **JWT engine ngắn hạn, downscoped** (RFC 8693 token-exchange), giao qua manifest F1 + endpoint refresh.
3. **egret-catalog validate JWT** + policy mặc định (`rw notebook_{user}`, group-read shared); giữ opaque `catalog_tokens` cho service/engine (hybrid).
4. **Trino**: JWT auth (JWKS Keycloak) + **authz native theo group** (file-based ACL; không Ranger ở MVP).
5. **StarRocks**: `CREATE SECURITY INTEGRATION authentication_jwt` + RBAC theo group.
6. **Daft seam**: connection factory + `RestCatalog` trình JWT, lấy token tươi từ egret mỗi connection.
7. **Test end-to-end trên Keycloak thật**, chứng minh biên authz per-user.

**Ngoài phạm vi:** Apache Ranger (dùng native authz theo group; epic ops riêng); **vended S3 credentials per-user** (§7 bản EN — JWT chỉ governs metadata/authz; đọc data file MinIO cần vended creds → F2-P6 tuỳ chọn hoặc spec B); **UX execution** notebook (spec E).

## Kiến trúc (tóm tắt)
Keycloak (1 issuer/JWKS, group claims) → user login egret → egret token-exchange → **JWT engine ngắn hạn** → kernel nhận qua manifest → connection factory trình JWT tới: egret-catalog (validate JWT) / Trino (JWT auth + ACL) / StarRocks (JWT sec-integration). Mọi service validate cùng token đã ký (iss+aud+JWKS), map principal/groups sang authz riêng.

## Quyết định chính
- **Issuer = Keycloak** (IdP ngoài, đã có trong dev-stack + egret OIDC login). egret = confidential client + token broker, không phải issuer.
- **Token kernel** = JWT ngắn hạn, downscoped qua **RFC 8693**; refresh theo nhu cầu; **không** dùng full SSO token, **không** principal đặc quyền.
- **Audience**: căn token shared/multi-aud; `required_audience` mỗi service khớp.
- **Authz**: **native theo Keycloak group** (Trino file ACL, StarRocks RBAC, catalog default+group). Ranger để sau.
- **Catalog auth**: **hybrid** — JWT cho user (mặc định `notebook_{user}` rw), opaque token cho service/engine.
- **Sửa** mô hình access của notebook: "1-grant catalog token" → "OIDC JWT delegation".

## 2 rủi ro tích hợp (có fallback)
1. **StarRocks client plugin** (§4.5 EN): cần spike xác nhận pymysql trình JWT theo `AUTHENTICATION_OPENID_CONNECT_CLIENT`; chưa xong → StarRocks SQL tạm dùng callback D1a.
2. **Audience** (§4.1/§6 EN): căn `aud` khớp 3 service.

## Test
- Unit Rust `#[sqlx::test]`: token-broker (mock Keycloak), catalog `front_auth` nhánh JWT (valid/expired/sai-aud/sai-iss) + authz mặc định (rw của mình; 403 namespace người khác); endpoint manifest/refresh.
- Unit pytest (mở rộng D1a): connection factory dựng connection mang JWT; `catalog()` truyền JWT + refresh khi 401.
- **E2E Keycloak thật (scripted, không CI)**: dev-stack đầy đủ, 2 user khác group; user A đọc được `notebook_A`/schema được cấp; user B **bị 403** với `notebook_A`/object không có quyền; token kernel chỉ là token downscoped; `aud`/`iss` giả → 401.
- **Security review đối kháng (bắt buộc — feature ăn tiền)**: token confusion/replay, lạm dụng audience, kernel với tới credential, xử lý refresh token, downscope đúng, thử bypass ACL.

## Phase
- **F2-P1** Keycloak realm + egret token broker (exchange + manifest + refresh).
- **F2-P2** egret-catalog JWT authn + authz mặc định/group (hybrid).
- **F2-P3** Trino JWT auth + file ACL theo group + Daft Trino factory.
- **F2-P4** StarRocks JWT security integration + RBAC + Daft StarRocks factory (gồm spike client plugin).
- **F2-P5** E2E Keycloak thật + security review đối kháng.
- **F2-P6 (tuỳ chọn)** vended S3 credentials per-user (§7).
