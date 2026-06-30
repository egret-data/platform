-- F2-P1: per-user OIDC refresh token, stored as a reference to an encrypted
-- vault secret (secrets.id). One row per user; replaced on each login.
CREATE TABLE oidc_refresh_tokens (
    user_id    uuid PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    secret_id  uuid NOT NULL REFERENCES secrets(id) ON DELETE CASCADE,
    updated_at timestamptz NOT NULL DEFAULT now()
);
