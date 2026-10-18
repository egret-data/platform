-- 0195_perm_engine_grants.sql
-- F11: engine-scoped grants (docs/en/specs/2026-10-18-perm-engine-grants-design.md).
--
-- An engine grant is an allow-only, include-only capability on ONE engine
-- (impersonate a named user, set a named session property, execute a named
-- procedure or function on Trino; impersonate, use a global function, manage
-- blacklists on StarRocks). `egret-perm-compile` compiles each row into the
-- engine's Ranger profile as `perm-engine-{id}`. Not `perm.grants`: that table
-- is catalog-scoped and every reader of it assumes so (spec §3.1).
--
-- FORMAT NOTE: `engine_grant_prefix_rename.rs` extracts the rename statement at
-- the bottom of this file and strips only WHOLE-LINE `--` comments, so keep
-- every comment on its own line.

CREATE TABLE perm.engine_grants (
    id          uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    engine_id   uuid NOT NULL REFERENCES engine_mgmt.engines(id) ON DELETE CASCADE,
    -- The curated catalogue (`egret_perm_compile::engine_capabilities`). The
    -- Rust table and this list are one fact in two places; a unit test reads
    -- this list back with `include_str!` and compares.
    capability  text NOT NULL CHECK (capability IN (
                    'trino.impersonate_user', 'trino.set_system_session_property',
                    'trino.set_catalog_session_property', 'trino.execute_procedure',
                    'trino.execute_function', 'starrocks.impersonate_user',
                    'starrocks.use_global_function', 'starrocks.manage_blacklist')),
    -- The capability's resource values other than an impersonation target, in
    -- the catalogue's key order. Concrete only: no Ranger wildcard (`*`, `?`)
    -- and no macro marker (`{`, `}`), so a grant never names more than it says.
    resource_values text[] NOT NULL,
    -- The impersonation target, by IDENTITY, never by name (spec E21, amended
    -- 2026-10-18). A username freed by a hard delete can be minted again for
    -- another person; a target stored by name would hand that person to the
    -- grantee. The compiler renders the CURRENT `users.username` from this id,
    -- and deleting the user deletes the grant.
    target_user_id uuid REFERENCES users(id) ON DELETE CASCADE,
    created_at  timestamptz NOT NULL DEFAULT now(),
    updated_at  timestamptz NOT NULL DEFAULT now(),
    CHECK (array_position(resource_values, NULL) IS NULL),
    CHECK (array_to_string(resource_values, chr(31)) !~ '[*?{}]'),
    CHECK (cardinality(resource_values) = CASE capability
             WHEN 'trino.set_catalog_session_property' THEN 2
             WHEN 'trino.execute_procedure' THEN 3
             WHEN 'trino.execute_function' THEN 3
             WHEN 'trino.impersonate_user' THEN 0
             WHEN 'starrocks.impersonate_user' THEN 0
             WHEN 'starrocks.manage_blacklist' THEN 0
             ELSE 1 END),
    CHECK ((capability IN ('trino.impersonate_user', 'starrocks.impersonate_user'))
           = (target_user_id IS NOT NULL)),
    UNIQUE NULLS NOT DISTINCT (engine_id, capability, resource_values, target_user_id)
);
CREATE INDEX engine_grants_engine_idx ON perm.engine_grants (engine_id);
CREATE INDEX engine_grants_target_user_idx ON perm.engine_grants (target_user_id)
    WHERE target_user_id IS NOT NULL;

CREATE TABLE perm.engine_grant_principals (
    engine_grant_id uuid NOT NULL REFERENCES perm.engine_grants(id) ON DELETE CASCADE,
    -- Engines are not workspace-scoped, and an engine capability for everyone
    -- is what the seeded baselines are for (spec E5).
    kind  perm.principal_kind NOT NULL CHECK (kind IN ('user', 'group', 'role')),
    ref   text NOT NULL CHECK (ref <> ''),
    CHECK (NOT (kind = 'group' AND ref = 'public')),
    PRIMARY KEY (engine_grant_id, kind, ref)
);
CREATE INDEX engine_grant_principals_ref_idx ON perm.engine_grant_principals (kind, ref);

-- Reserve the prefix (spec E19). Before this migration `perm-engine-` was an
-- ordinary name an operator could give a policy; after it, the compiler's sweep
-- reaps every policy under that prefix that no engine grant produces. Rename
-- every such row that the compiler does not own, in the column and in the body,
-- and bump the row's version and its profile's `policy_version` so engines
-- download the new name. No compiler-owned row can carry the prefix yet. The
-- prefix holds no `_` or `%`, so LIKE is exact here.
WITH renamed AS (
    UPDATE policy.authz_policies
       SET name = 'renamed by 0195: ' || name,
           policy_jsonb = jsonb_set(policy_jsonb, '{name}', to_jsonb('renamed by 0195: ' || name)),
           version = version + 1,
           updated_at = now()
     WHERE name LIKE 'perm-engine-%'
       AND managed_by <> 'compiler'
    RETURNING service_id
)
UPDATE policy.authz_services
   SET policy_version = policy_version + 1,
       updated_at = now()
 WHERE id IN (SELECT service_id FROM renamed);

DO $$
DECLARE
    n bigint;
BEGIN
    SELECT count(*) INTO n FROM policy.authz_policies WHERE name LIKE 'renamed by 0195: %';
    RAISE NOTICE '0195: % policy name(s) under the reserved prefix perm-engine- renamed', n;
END
$$;
