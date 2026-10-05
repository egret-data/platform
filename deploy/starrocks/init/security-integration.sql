-- F2-P4 / P4b: authenticate per-user sessions to StarRocks with the OIDC engine JWT.
-- VERIFIED LIVE 2026-07-01 against starrocks/allin1-ubuntu (StarRocks 4.1.1) with the
-- patched vendored mysql_async (third_party/) driver.
--
-- Apply manually against the FE (port 9030) as root, or via an init step (see README).
-- JWKS + issuer + audience must match Keycloak realm `egret` and the `starrocks`
-- audience mapper added in F2-P1.
--
-- IMPORTANT (JwkMgr): `jwks_url` MUST start with http:// or https:// — StarRocks fetches
-- it as a URL. Any other value is treated as a FILE relative to $STARROCKS_HOME/conf/
-- (a bare/empty value resolves to the conf directory → "Is a directory" auth error).

-- 1) The realm-wide security integration (used for the auth chain / group mapping).
CREATE SECURITY INTEGRATION egret_oidc PROPERTIES (
    "type" = "authentication_jwt",
    "jwks_url" = "http://keycloak:8080/realms/egret/protocol/openid-connect/certs",
    "principal_field" = "preferred_username",
    "required_issuer" = "http://localhost:8081/realms/egret",
    "required_audience" = "starrocks"
);

-- 2) Activate it in the FE authentication chain (verified working on 4.1.1):
ADMIN SET FRONTEND CONFIG ("authentication_chain" = "native,egret_oidc");

-- 3) Per-user binding (verified working). A user must be IDENTIFIED WITH authentication_jwt
--    for StarRocks to request the `authentication_openid_connect_client` plugin AND carry
--    the JWT validation config. `principal_field=preferred_username` maps the token to this
--    StarRocks user (the Keycloak username). Provision one per user; grant as needed.
CREATE USER 'tester'@'%' IDENTIFIED WITH authentication_jwt AS '{
    "jwks_url": "http://keycloak:8080/realms/egret/protocol/openid-connect/certs",
    "principal_field": "preferred_username",
    "required_issuer": "http://localhost:8081/realms/egret",
    "required_audience": "starrocks"
}';
GRANT OPERATE ON SYSTEM TO 'tester'@'%';   -- replace with the real per-user grants

-- Verified live: a valid Keycloak access token (aud contains `starrocks`) authenticates as
-- the mapped user (SELECT current_user() → 'tester'@'%'); a garbage or signature-tampered
-- token is REJECTED (JWE/kid invalid). Auth ran over plain TCP in dev; TLS is a prod
-- hardening item (the OpenID client protocol expects a secure transport). See README.md.
