CREATE EXTENSION IF NOT EXISTS citext;

CREATE TABLE users (
    id              uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    provider        text NOT NULL CHECK (provider IN ('local','oidc')),
    external_subject text,
    email           citext NOT NULL UNIQUE,
    name            text NOT NULL DEFAULT '',
    password_hash   text,
    status          text NOT NULL DEFAULT 'active' CHECK (status IN ('active','disabled')),
    created_at      timestamptz NOT NULL DEFAULT now(),
    last_login_at   timestamptz
);

CREATE TABLE groups (
    id     uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    name   text NOT NULL UNIQUE,
    source text NOT NULL DEFAULT 'local' CHECK (source IN ('local','oidc'))
);

CREATE TABLE user_groups (
    user_id  uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    group_id uuid NOT NULL REFERENCES groups(id) ON DELETE CASCADE,
    PRIMARY KEY (user_id, group_id)
);

CREATE TABLE sessions (
    id                  uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    token_hash          text NOT NULL UNIQUE,
    user_id             uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at          timestamptz NOT NULL DEFAULT now(),
    absolute_expires_at timestamptz NOT NULL,
    idle_expires_at     timestamptz NOT NULL,
    last_seen_at        timestamptz NOT NULL DEFAULT now(),
    user_agent          text NOT NULL DEFAULT '',
    ip                  text NOT NULL DEFAULT ''
);

CREATE INDEX sessions_user_id_idx ON sessions(user_id);
