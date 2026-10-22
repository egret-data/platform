-- KI-185: a workspace admin can add an SSO user by email before that user's
-- first sign-in.
--
-- Egret only learns an SSO user at their first sign-in (JIT provisioning keyed
-- on the IdP `sub`), so until then the member picker had nobody to add and the
-- user landed on "No workspaces yet". A row here is a membership waiting for
-- its user: the OIDC callback turns it into an `access_grants` row when a user
-- whose IdP asserts `email_verified = true` for this address signs in, then
-- deletes it.
--
-- What a pre-registration can grant is limited by the schema, not only by the
-- handler: `role_scope` is pinned to 'workspace' and is half of a foreign key
-- to `roles (id, scope)`, so a row naming a platform role cannot exist.
--
-- `invited_by` cascades: an invitation from an account that has been deleted
-- grants nothing.

ALTER TABLE roles ADD CONSTRAINT roles_id_scope_key UNIQUE (id, scope);

CREATE TABLE workspace_invites (
    id           uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    workspace_id uuid NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    email        citext NOT NULL,
    role_id      uuid NOT NULL,
    role_scope   text NOT NULL DEFAULT 'workspace' CHECK (role_scope = 'workspace'),
    invited_by   uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at   timestamptz NOT NULL DEFAULT now(),
    FOREIGN KEY (role_id, role_scope) REFERENCES roles (id, scope) ON DELETE CASCADE,
    UNIQUE (workspace_id, email)
);

CREATE INDEX workspace_invites_email_idx ON workspace_invites (email);
