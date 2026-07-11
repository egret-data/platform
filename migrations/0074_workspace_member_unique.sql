-- B2-flip hardening (security review finding B): restore the DB-enforced
-- one-workspace-role-per-member invariant that the legacy `workspace_members` PK
-- provided. After the flip, `MemberRepo::add` uses an app-level DELETE+INSERT in a
-- READ COMMITTED tx; two concurrent admin `add` calls for the same (principal, ws)
-- could write-skew into TWO workspace roles for one member (PDP unions → the higher
-- role wins). A partial unique index makes the second concurrent INSERT fail
-- (unique violation → 500) instead of silently producing a two-role member — the
-- same trade-off already used for platform grants (0073 access_grants_platform_uniq).

-- Self-heal: collapse any pre-existing duplicate workspace grants for the same
-- (principal, workspace) before the index (residue from the write-skew window or
-- test seeds). Extra rows are exact duplicates on the indexed key; keep one.
DELETE FROM access_grants a USING access_grants b
WHERE a.scope_type = 'workspace' AND b.scope_type = 'workspace'
  AND a.effect = 'allow' AND b.effect = 'allow'
  AND a.principal_type = b.principal_type
  AND a.principal_id = b.principal_id
  AND a.scope_id = b.scope_id
  AND a.ctid > b.ctid;

-- One allow-grant per (principal, workspace) at workspace scope.
CREATE UNIQUE INDEX access_grants_ws_member_uniq
  ON access_grants (principal_type, principal_id, scope_id)
  WHERE scope_type = 'workspace' AND effect = 'allow';
