-- egret mini machine-IdP signing keys (rotatable; one active at a time).
CREATE TABLE oidc_signing_keys (
    kid          text PRIMARY KEY,
    algorithm    text NOT NULL DEFAULT 'RS256',
    private_pem  text NOT NULL,
    public_jwk   jsonb NOT NULL,
    created_at   timestamptz NOT NULL DEFAULT now(),
    retired_at   timestamptz
);
