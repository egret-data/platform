-- An active impersonation overlay on an admin's session. Absence = the session
-- is the admin acting as themselves. At most one active row per admin session.
CREATE TABLE impersonations (
    id                 UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    admin_session_id   UUID        NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    impersonator_id    UUID        NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    target_user_id     UUID        NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    -- NULL = unrestricted (platform_admin). Non-NULL = ws_admin, confined here.
    workspace_scope_id UUID        REFERENCES workspaces(id) ON DELETE CASCADE,
    reason             TEXT        NOT NULL,
    started_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at         TIMESTAMPTZ NOT NULL
);

CREATE UNIQUE INDEX impersonations_one_per_session ON impersonations (admin_session_id);

-- Every audited mutation made while impersonating records the real admin behind
-- the target identity it ran as. NULL for ordinary (non-impersonated) actions.
ALTER TABLE audit.audit_log ADD COLUMN impersonator UUID;
