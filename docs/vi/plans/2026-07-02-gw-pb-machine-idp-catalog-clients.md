# Gateway P-B: egret mini machine-IdP + Catalog Clients (bản tham chiếu)

> Bản chuẩn: `docs/en/plans/2026-07-02-gw-pb-machine-idp-catalog-clients.md`. Spec Trụ 2. Thực thi qua subagent-driven-development. **Nền cho machine auth.**

**Mục tiêu:** OAuth2/OIDC provider nhúng, egret sở hữu: `POST /oauth/token` (client_credentials) → JWT egret ký, + `/.well-known/{openid-configuration,jwks.json}` để Polaris verify. Backed bởi registry `catalog_clients` (tạo → clientId/secret 1 lần, revoke riêng).

**Kiến trúc:** crate mới `egret-machine-idp` giữ signing key (lưu, rotatable) + JWKS/discovery + token endpoint. `catalog_clients` (trong egret-controlplane) = client store; tạo client gen clientId/secret (hash secret, hiện 1 lần) + ghi `(consumer_type, workspace, catalog, catalog_role)`. JWT: `iss`, `sub=client_id`, `aud=polaris`, `roles:[catalog_role]`, `exp` ngắn. Trust Polaris ở P-D.

## Bối cảnh
- Migration tiếp `0052`; touch metastore. egret-controlplane có CpState/guard/CpError/polaris_client. `egret-machine-idp` = crate mới (public, không platform-admin gate); verify client qua trait (không phụ thuộc egret-controlplane).
- JWT: `jsonwebtoken` RS256 + `kid`. JWKS từ public RSA (n/e base64url). Key RSA-2048 (`rsa` crate), lưu PEM.

## Task
- **T1 (migration 0052):** `catalog_clients { id, name, consumer_type CHECK(trino|starrocks|notebook|external), workspace_id→workspaces, catalog_name, catalog_role, client_id UNIQUE, client_secret_hash, created_at, last_used_at }` + `oidc_signing_keys { kid PK, algorithm, private_pem, public_jwk jsonb, created_at, retired_at }` + `DROP workspace_catalog_assignments`. migrate+verify.
- **T2 (signing keys + JWKS/discovery):** crate `egret-machine-idp`; `SigningKeyStore` trait (`active`, `all_public_jwks`) + `PgSigningKeyStore`; `keys.rs` gen RSA→PEM+JWK; `token.rs` sign/verify (RS256+kid); routes `/.well-known/openid-configuration` + `/jwks.json`. Test gen key→JWK+round-trip sign/verify.
- **T3 (`/oauth/token`):** `ClientVerifier` trait (`verify→Option<VerifiedClient{client_id,catalog,catalog_role}>`); handler `client_credentials`→verify→`sign_claims({iss,sub,aud:polaris,roles:[catalog_role],catalog,exp})`; sai secret→401 (`invalid_client`), grant khác→400. Không log secret. Test mint JWT + claims + 401.
- **T4 (catalog_clients store + routes):** `CatalogClientStore.create` (gen client_id `egret_`+hex, secret 32B base64url, hash **argon2**, INSERT; trước đó `ensure` catalog-role tồn tại trong Polaris qua `polaris_client.create_catalog_role`+ignore_conflict) / `verify` (argon2 constant-time + update last_used) / `list_for_catalog` / `list_for_workspace` / `revoke`; impl `ClientVerifier`. Routes (platform_admin): `POST /polaris/catalogs/{catalog}/clients`→201 (secret 1 lần), `GET .../clients`, `DELETE /polaris/clients/{id}`. Test create→verify + 403.
- **T5 (wire+verify):** `IdpState{keys:PgSigningKeyStore, issuer:public root, clients:CatalogClientStore}`; nest `idp_router` ở root **không CSRF/auth**; `issuer` = public root egret (khớp P-D + `iss`). Smoke: discovery/jwks/token qua curl; decode JWT (aud=polaris, roles, catalog, iss). fmt+clippy+test.

## Ghi chú
- Bảo mật: secret argon2 + hiện 1 lần + verify constant-time; JWT `aud=polaris`+exp ngắn+`kid`; không log token/secret.
- Cross-plan: `roles` claim map sang Polaris catalog-role — *tồn tại* role ensure ở đây (T4), *grants* ở P-C, *trust+mapping* Polaris ở P-D.
- Ngoài P-B: proxy (P-A), RBAC grants UI (P-C), config trust Polaris (P-D).
