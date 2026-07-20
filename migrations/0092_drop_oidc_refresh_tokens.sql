-- STS cleanup: sub-project 1 (#304) replaced the Keycloak refresh-grant
-- engine-token minter with egret's machine-IdP. The OIDC login callback no
-- longer needs to persist a refresh token (no reader remains), so drop the
-- now-dead table.
DROP TABLE IF EXISTS oidc_refresh_tokens;
