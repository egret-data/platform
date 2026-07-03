# EB-2: egret mini machine-IdP (bản tham chiếu)

> Bản chuẩn: `docs/en/plans/2026-07-02-eb2-machine-idp.md`. Spec Component 2. subagent-driven.

**Mục tiêu:** OAuth2 provider nhúng, egret sở hữu: `POST /oauth/token` (client_credentials)→JWT egret ký + `/.well-known/{openid-configuration,jwks.json}` để lakekeeper nhúng (EB-3) validate. `catalog_clients` + impl `ClientVerifier` ở EB-3 (tham chiếu warehouses/roles của EB-3); EB-2 định nghĩa IdP + seam `ClientVerifier`.

**Kiến trúc:** crate `egret-machine-idp`: RSA signing key (lưu `oidc_signing_keys`, rotatable) + JWKS/discovery + token endpoint (verify client qua trait inject + ký JWT `sub`/`warehouse`/`roles`/`aud`/`exp`).

## Task
- **T1 (migration 0052):** `oidc_signing_keys { kid PK, algorithm, private_pem, public_jwk jsonb, created_at, retired_at }`. touch metastore + migrate + verify.
- **T2 (scaffold+keys+JWKS):** crate (jsonwebtoken/rsa/rand/serde/base64/axum/sqlx…); `keys.generate_rsa_key`→KeyMaterial(kid,pem,jwk); `token.sign_claims`/`verify_with_jwk`; `SigningKeyStore`+`PgSigningKeyStore` (active→tạo nếu chưa; all_public_jwks); `IdpState`+`idp_router` (discovery+jwks+token). Test gen key round-trip.
- **T3 (`/oauth/token`):** `ClientVerifier` (`verify→Option<VerifiedClient{client_id,warehouse,catalog_role}>`); handler client_credentials→verify→sign `{iss,sub,aud:"catalog",warehouse,roles:[catalog_role],exp}`; sai secret→401 `invalid_client`. Không log secret. Test mint JWT + claims + 401.
- **T4 (mount):** `IdpState{keys:PgSigningKeyStore, issuer:public root, clients: placeholder AllowNone (EB-3 swap CatalogClientStore)}`; nest `idp_router` root **không CSRF/auth**; `issuer`=public root (khớp EB-3+`iss`). Smoke discovery/jwks/token(invalid_client). fmt+clippy+test.

## Ghi chú
- Security: JWT `aud`/`exp`/`kid`; không log token/secret.
- Ngoài EB-2: client store+warehouses/roles/grants+Authorizer+trust lakekeeper (EB-3), mgmt (EB-4), UI (EB-5), e2e (EB-6).
