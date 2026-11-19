-- EO5: begin the soak. Fold existing grants into the mirror so it is complete
-- from the first boot, then seed both reversible switches to `shadow` (observe):
-- serve the proven legacy decisions while comparing the new paths. ON CONFLICT
-- DO NOTHING never clobbers an operator's explicit choice; an absent/unreadable
-- row still falls back to `legacy` in code, so `shadow` is the default and
-- `legacy` the fail-safe floor.

-- EO4's switch (0239) was created with CHECK (source IN ('legacy','unified')) —
-- it has no `shadow`. Widen it to allow the new observe state before seeding it
-- (EO3's 0238 switch already allows 'shadow'). The constraint's auto-generated
-- name is <table>_<column>_check; drop IF EXISTS to be robust.
ALTER TABLE perm.cedar_grant_source DROP CONSTRAINT IF EXISTS cedar_grant_source_source_check;
ALTER TABLE perm.cedar_grant_source
    ADD CONSTRAINT cedar_grant_source_source_check CHECK (source IN ('legacy', 'shadow', 'unified'));

INSERT INTO perm.cedar_grants
    (id, principal_type, principal_id, role_id, scope_type, scope_id, object_type, effect, expires_at)
SELECT id, principal_type, principal_id, role_id, scope_type, scope_id, object_type, effect, expires_at
FROM access_grants
ON CONFLICT (id) DO UPDATE SET
    principal_type = EXCLUDED.principal_type, principal_id = EXCLUDED.principal_id,
    role_id = EXCLUDED.role_id, scope_type = EXCLUDED.scope_type, scope_id = EXCLUDED.scope_id,
    object_type = EXCLUDED.object_type, effect = EXCLUDED.effect, expires_at = EXCLUDED.expires_at;

INSERT INTO perm.pdp_policy_source (id, source, switched_at, switched_by)
VALUES (true, 'shadow', now(), 'eo5-migration')
ON CONFLICT (id) DO NOTHING;

INSERT INTO perm.cedar_grant_source (id, source, switched_at, switched_by)
VALUES (true, 'shadow', now(), 'eo5-migration')
ON CONFLICT (id) DO NOTHING;
