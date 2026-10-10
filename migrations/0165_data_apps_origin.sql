-- Data Apps P4 (spec §6.1, §6.2, §5.3): the apps-origin grant, pod tokens, and the
-- app_sessions columns that trace a query token to a grant and an impersonator.

-- A grant is a ticket that has been, or may yet be, redeemed. The ticket is spent by
-- setting redeemed_at in ONE statement (UPDATE ... WHERE redeemed_at IS NULL), and the
-- apps cookie then carries the plaintext whose hash is cookie_hash. login_sid cascades
-- from egret's own login session, so logout and OIDC back-channel logout kill grants
-- without any code having to remember to.
CREATE TABLE analytics.app_grants (
    id                   uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    ticket_hash          text NOT NULL UNIQUE,
    cookie_hash          text NULL UNIQUE,
    user_id              uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    workspace_id         uuid NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    app_id               uuid NOT NULL,
    -- NULL = the draft (edit mode), as in app_sessions.
    version_id           uuid NULL,
    mode                 text NOT NULL CHECK (mode IN ('edit', 'run')),
    login_sid            uuid NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    impersonator_user_id uuid NULL REFERENCES users(id) ON DELETE SET NULL,
    created_at           timestamptz NOT NULL DEFAULT now(),
    ticket_expires_at    timestamptz NOT NULL,
    redeemed_at          timestamptz NULL,
    cookie_expires_at    timestamptz NULL,
    revoked_at           timestamptz NULL,
    CHECK ((mode = 'edit') = (version_id IS NULL)),
    -- spent and holding a cookie are one state, never two
    CHECK ((redeemed_at IS NULL) = (cookie_hash IS NULL)),
    CONSTRAINT app_grants_app_workspace_fk FOREIGN KEY (app_id, workspace_id)
        REFERENCES analytics.apps (id, workspace_id) ON DELETE CASCADE,
    CONSTRAINT app_grants_version_app_fk FOREIGN KEY (version_id, app_id)
        REFERENCES analytics.app_versions (id, app_id) ON DELETE CASCADE
);
CREATE INDEX app_grants_login_sid_idx ON analytics.app_grants (login_sid);
CREATE INDEX app_grants_user_id_idx ON analytics.app_grants (user_id);
CREATE INDEX app_grants_app_id_idx ON analytics.app_grants (app_id);
CREATE INDEX app_grants_workspace_id_idx ON analytics.app_grants (workspace_id);

-- Pod tokens (spec §5.3). An edit pod's token is bound to (user, workspace) and may
-- touch any app that user can write; a run pod's to exactly one (app, version).
-- Revoked when egret stops the pod.
CREATE TABLE analytics.app_pod_tokens (
    id           uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    token_hash   text NOT NULL UNIQUE,
    kind         text NOT NULL CHECK (kind IN ('edit', 'run')),
    user_id      uuid NULL REFERENCES users(id) ON DELETE CASCADE,
    workspace_id uuid NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    app_id       uuid NULL,
    version_id   uuid NULL,
    created_at   timestamptz NOT NULL DEFAULT now(),
    revoked_at   timestamptz NULL,
    CHECK (kind <> 'edit' OR (user_id IS NOT NULL AND app_id IS NULL AND version_id IS NULL)),
    CHECK (kind <> 'run'  OR (app_id IS NOT NULL AND version_id IS NOT NULL)),
    CONSTRAINT app_pod_tokens_app_workspace_fk FOREIGN KEY (app_id, workspace_id)
        REFERENCES analytics.apps (id, workspace_id) ON DELETE CASCADE,
    CONSTRAINT app_pod_tokens_version_app_fk FOREIGN KEY (version_id, app_id)
        REFERENCES analytics.app_versions (id, app_id) ON DELETE CASCADE
);
CREATE INDEX app_pod_tokens_workspace_id_idx ON analytics.app_pod_tokens (workspace_id);
CREATE INDEX app_pod_tokens_user_id_idx ON analytics.app_pod_tokens (user_id);

ALTER TABLE analytics.app_sessions
    ADD COLUMN grant_id uuid NULL REFERENCES analytics.app_grants (id) ON DELETE CASCADE,
    ADD COLUMN impersonator_user_id uuid NULL REFERENCES users (id) ON DELETE SET NULL;
CREATE INDEX app_sessions_grant_id_idx ON analytics.app_sessions (grant_id);
