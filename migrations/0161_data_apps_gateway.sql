-- Data Apps P1 (docs/en/specs/2026-09-30-marimo-data-apps-design.md §4, §5).
-- `apps` is the identity row the gateway authorizes against; drafts, versions and
-- the app API arrive in P2.
CREATE TABLE analytics.apps (
    id            uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    workspace_id  uuid NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    name          text NOT NULL CHECK (length(name) BETWEEN 1 AND 200),
    description   text NOT NULL DEFAULT '',
    -- P2 wires folders; until then every app is Uncategorized.
    folder_id     uuid NULL,
    owner_user_id uuid NULL REFERENCES users(id) ON DELETE SET NULL,
    created_at    timestamptz NOT NULL DEFAULT now(),
    updated_at    timestamptz NOT NULL DEFAULT now(),
    deleted_at    timestamptz NULL,
    -- target of app_sessions' composite FK: a session's workspace is its app's workspace
    CONSTRAINT apps_id_workspace_id_key UNIQUE (id, workspace_id)
);
CREATE INDEX apps_workspace_id_idx ON analytics.apps (workspace_id) WHERE deleted_at IS NULL;

-- One row per marimo session the apps proxy opens (spec §4.1). Only the SHA-256
-- of the token is stored. `login_sid` cascades from egret's own login session, so
-- logout — including OIDC back-channel logout, which deletes that row — revokes
-- every app token minted under it without any code having to remember to.
CREATE TABLE analytics.app_sessions (
    id                  uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    token_hash          text NOT NULL UNIQUE,
    user_id             uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    workspace_id        uuid NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    app_id              uuid NOT NULL,
    -- NULL = the draft (edit mode). The FK to analytics.app_versions is added in P2.
    version_id          uuid NULL,
    mode                text NOT NULL CHECK (mode IN ('edit', 'run')),
    login_sid           uuid NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    created_at          timestamptz NOT NULL DEFAULT now(),
    last_used_at        timestamptz NOT NULL DEFAULT now(),
    idle_expires_at     timestamptz NOT NULL,
    absolute_expires_at timestamptz NOT NULL,
    revoked_at          timestamptz NULL,
    -- edit always runs the draft; run always runs a pinned version
    CHECK ((mode = 'edit') = (version_id IS NULL)),
    CHECK (idle_expires_at <= absolute_expires_at),
    -- the schema, not just the gateway query, guarantees workspace_id is the app's workspace
    CONSTRAINT app_sessions_app_workspace_fk FOREIGN KEY (app_id, workspace_id)
        REFERENCES analytics.apps (id, workspace_id) ON DELETE CASCADE
);
CREATE INDEX app_sessions_login_sid_idx ON analytics.app_sessions (login_sid);
CREATE INDEX app_sessions_app_id_idx ON analytics.app_sessions (app_id);
CREATE INDEX app_sessions_user_id_idx ON analytics.app_sessions (user_id);
CREATE INDEX app_sessions_workspace_id_idx ON analytics.app_sessions (workspace_id);
