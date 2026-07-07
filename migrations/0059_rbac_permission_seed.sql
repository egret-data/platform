-- data_steward: read across the platform + full policy control.
INSERT INTO role_permissions (role_id, permission)
SELECT r.id, perm FROM roles r, unnest(ARRAY[
    'policy:read','policy:create','policy:update','policy:delete','policy:seed',
    'warehouse:read','storage:read','engine:read',
    'user:read','group:read','role:read'
]) AS perm
WHERE r.scope='platform' AND r.name='data_steward'
ON CONFLICT (role_id, permission) DO NOTHING;
