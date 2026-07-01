-- F2-P4: authenticate notebook users to StarRocks with the OIDC engine JWT.
-- Apply manually against the FE (port 9030) as root, or via an init step (see README).
-- JWKS + issuer + audience must match Keycloak realm `egret` and the `starrocks`
-- audience mapper added in F2-P1.
CREATE SECURITY INTEGRATION egret_oidc PROPERTIES (
    "type" = "authentication_jwt",
    "jwks_url" = "http://keycloak:8080/realms/egret/protocol/openid-connect/certs",
    "principal_field" = "sub",
    "required_issuer" = "http://keycloak:8080/realms/egret",
    "required_audience" = "starrocks"
);

-- Activating the integration (authentication chain / group provider) is
-- StarRocks-version-specific — finalized at P5 against the allin1-ubuntu image.
--
-- Possible activation forms (confirm the correct syntax in P5):
--
-- Option A — set authentication_chain at the FE level via ADMIN SET FRONTEND CONFIG:
--   ADMIN SET FRONTEND CONFIG ("authentication_chain" = "egret_oidc,native");
--
-- Option B — if the running version supports ALTER SYSTEM for security integrations:
--   ALTER SYSTEM SET SECURITY INTEGRATION egret_oidc;
--
-- Option C — group provider (if StarRocks exposes a GROUP PROVIDER DDL):
--   CREATE GROUP PROVIDER egret_groups
--     USING SECURITY INTEGRATION egret_oidc
--     PROPERTIES ("groups_claim" = "groups");
--
-- Do NOT run any of these uncommented until the exact syntax is confirmed at P5.
