# egret Polaris Gateway + Machine-IdP — Thiết kế (bản tham chiếu)

> Bản chuẩn: `docs/en/specs/2026-07-02-polaris-gateway-machine-idp-design.md`. Kế thừa/sửa `2026-07-02-control-plane-polaris-catalog-design.md` (control-plane đã merge ở PR #115).

## Động cơ (4 điểm sửa từ review live)
1. **Expose Polaris qua egret** — consumer tới catalog qua 1 edge egret; Polaris không lộ trực tiếp.
2. **RBAC nằm trong catalog** — catalog-role + grant là per-catalog; trang `/rbac` phẳng sai ngữ nghĩa.
3. **Surface machine credential** — "internal client" phải là entity hạng nhất, không chôn trong Secrets.
4. **KHÔNG buộc credential máy vào IdP doanh nghiệp** (EntraID phải qua security — bất khả thi); và Polaris `/v1/oauth/tokens` deprecated → không bền.

## Nguyên tắc lõi — 3 mặt phẳng danh tính, 3 issuer
| Mặt phẳng | Danh tính | Issuer | Ai cấp |
|---|---|---|---|
| **Người** (login egret, governance per-user ở engine) | user thật | **IdP doanh nghiệp** (EntraID/Keycloak họ) — egret đã OIDC login | security (SSO) |
| **Máy** (Catalog Client: engine/tool/notebook → catalog) | service | **egret mini machine-IdP** (nhúng, egret sở hữu) | egret, tự do |
| **AuthZ** (máy được làm gì trên catalog) | principal/role+grant | **Polaris** (RBAC) | control-plane egret, server-side |
→ **AuthN máy = egret; authZ = Polaris; SSO người = IdP doanh nghiệp.** Credential máy không đụng EntraID; consumer không gọi token endpoint deprecated của Polaris.

## Quyết định đã chốt
- Consumers: engines + external/BI + notebook(Daft), không per-user trực tiếp.
- Credential: **1 / consumer** (Catalog Client), revoke/audit riêng.
- Proxy: chỉ data-plane `/catalog/v1/*` (management giữ nội bộ); Polaris ẩn; proxy **không** auth.
- Machine auth: **egret mini machine-IdP** (không Polaris-native, không Keycloak, không EntraID).

## 4 trụ
**Trụ 1 — Proxy trong suốt `/catalog/v1/*`:** forward method+path+`Authorization`+body sang `{polaris}/api/catalog/v1/*`; không chèn auth. Rewrite URL trong `GET /v1/config` (`overrides.uri`, `s3.signer.uri`) về host egret. Polaris chỉ mạng nội bộ.

**Trụ 2 — egret mini machine-IdP + Catalog Clients:**
- `POST /oauth/token` (client_credentials) → ký JWT (`iss`=egret, `sub`=client, `aud=polaris`, claim roles, TTL ngắn). `GET /.well-known/openid-configuration` + `/jwks.json`. Ký RS256, key egret tự sinh+lưu, rotatable, `jsonwebtoken`. Không service ngoài.
- `catalog_clients { id, name, consumer_type, workspace_id, catalog_name, catalog_role, client_id, client_secret_hash, created_at, last_used_at }`. Tạo→gen clientId/secret (hiện 1 lần) + đảm bảo authZ object trong Polaris; revoke = xoá row. Tập distinct `(ws, catalog)` **thay** bảng `workspace_catalog_assignments`.

**Trụ 3 — RBAC theo catalog:** bỏ trang `/rbac` phẳng. Trong `/control-plane/catalog/{name}`: Catalog roles + Grants + Clients (mỗi client gắn 1 catalog-role). Global chỉ overview read-only.

**Trụ 4 — Polaris EXTERNAL-OIDC tin egret:** `authentication.type=external|mixed`, `quarkus.oidc.auth-server-url={egret issuer}`, `token.audience=polaris` → validate JWT egret qua JWKS. Mapping (spike): (a) `roles` claim→catalog-role *(mặc định, ít object)* hoặc (b) `sub`→principal. egret-admin token: từ chính egret mini-IdP (client admin) hoặc bootstrap root (dev).

## Data model
- Mới: `catalog_clients` + `oidc_signing_keys` (hoặc file bảo mật).
- Bỏ: `workspace_catalog_assignments` (gộp vào clients). Field kc-token của connection thành optional.

## Kế thừa/sửa PR #115
- Assignment mint principal-native → thay bằng Catalog Client + egret mini-IdP.
- `/rbac` phẳng → RBAC trong catalog detail.
- Polaris dev INTERNAL(root) → EXTERNAL-OIDC trỏ egret (dev có thể giữ root cho admin path).
- `egret-polaris` client (management) **giữ** — vẫn dùng server-side.

## Bảo mật
- `client_secret` hash (argon2/bcrypt), hiện 1 lần. JWT TTL ngắn, `aud=polaris`, JWKS rotatable (`kid`). Proxy không log token. `/oauth/token` rate-limit + so sánh secret constant-time.

## Ngoài scope / future
- Un-pause notebook Daft-direct (sub-epic). Auto-đẩy credential vào engine (future). Per-user OIDC-to-Polaris + governance cột/dòng (ở engine/Ranger). IdP doanh nghiệp cho máy (đã loại).

## Rủi ro
- **Polaris OIDC mapping** (trụ 4) chưa-pin nhất — spike ngắn vs `apache/polaris` trước khi chốt routes/e2e.
- Vận hành IdP nhúng = thêm vòng đời signing-key + JWKS/discovery (bounded).

## Phân rã (1 plan / trụ)
- **P-A** proxy `/catalog/v1/*` + rewrite `/v1/config`.
- **P-B** egret mini machine-IdP + `catalog_clients`.
- **P-C** RBAC UI theo catalog (bỏ `/rbac`, gộp assignment).
- **P-D** Polaris EXTERNAL-OIDC (compose) + spike mapping + e2e (client→token→proxy→Polaris→MinIO).
