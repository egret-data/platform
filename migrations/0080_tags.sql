-- Classification (tag) model — the metadata foundation for ABAC (Access P3.1).
-- METADATA ONLY: a `tags` catalog + `tag_assignments` (which objects carry which
-- tags). No authorization decision reads these tables yet (enforcement is a later
-- phase).

CREATE TABLE tags (
    id          uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    key         text NOT NULL UNIQUE,
    name        text NOT NULL,
    description text NOT NULL DEFAULT '',
    color       text NOT NULL DEFAULT '',
    created_by  uuid REFERENCES users(id),
    created_at  timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE tag_assignments (
    tag_id      uuid NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
    object_type text NOT NULL,
    object_id   uuid NOT NULL,
    assigned_by uuid REFERENCES users(id),
    assigned_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (tag_id, object_type, object_id)
);

CREATE INDEX tag_assignments_object_idx ON tag_assignments (object_type, object_id);

-- Grant data_steward the platform-scoped classification perms (mirrors the
-- policy:* seed in 0059: data_steward reads across the platform + governs).
INSERT INTO role_permissions (role_id, permission)
SELECT r.id, perm FROM roles r, unnest(ARRAY[
    'tag:read','tag:manage'
]) AS perm
WHERE r.scope='platform' AND r.name='data_steward'
ON CONFLICT (role_id, permission) DO NOTHING;
