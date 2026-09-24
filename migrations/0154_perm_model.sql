-- 0154_perm_model.sql
-- SP4a. egret's first data-permission model. `policy.*` keeps serving raw
-- Ranger JSON for hand-written profiles; `perm.*` is the model that COMPILES
-- to it. One direction only — nothing reads Ranger JSON back into perm.
CREATE SCHEMA perm;

CREATE TYPE perm.effect AS ENUM ('allow', 'deny');
CREATE TYPE perm.principal_kind AS ENUM ('user', 'group', 'role', 'workspace_members');
CREATE TYPE perm.principal_mode AS ENUM ('include', 'exclude');
CREATE TYPE perm.rule_kind AS ENUM ('mask', 'row_filter');
CREATE TYPE perm.selector_kind AS ENUM ('tag', 'path');

CREATE TABLE perm.grants (
    id          uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    effect      perm.effect NOT NULL,
    -- SP4a ships exactly two actions (global constraints). The vocabulary
    -- lives here, not only in Rust: a junk row would otherwise make every
    -- OTHER row's read fail too (Action::parse turns a bad element into an
    -- Err that Vec<Result<_,_>>::collect propagates for the whole batch),
    -- so one bad write must never reach the table.
    actions     text[] NOT NULL
                CHECK (cardinality(actions) > 0)
                CHECK (actions <@ ARRAY['read', 'write']),
    catalog_id  uuid NOT NULL REFERENCES registry.catalogs(id) ON DELETE CASCADE,
    schema_name text,
    table_name  text,
    column_name text,
    -- `workspace_assignment` rows are written by the assignment path, not by a
    -- human, and are recreated from the assignment on demand (D3).
    origin      text NOT NULL DEFAULT 'explicit'
                CHECK (origin IN ('explicit', 'workspace_assignment')),
    origin_workspace_id uuid REFERENCES workspaces(id) ON DELETE CASCADE,
    created_at  timestamptz NOT NULL DEFAULT now(),
    -- A path cannot skip a level: a column implies a table implies a schema.
    CHECK (schema_name IS NOT NULL OR table_name IS NULL),
    CHECK (table_name IS NOT NULL OR column_name IS NULL),
    -- An implicit grant names its workspace; an explicit one does not.
    CHECK ((origin = 'workspace_assignment') = (origin_workspace_id IS NOT NULL))
);
CREATE INDEX grants_catalog_idx ON perm.grants (catalog_id);
-- One implicit grant per (catalog, workspace): the assignment is the identity.
CREATE UNIQUE INDEX grants_one_per_assignment
    ON perm.grants (catalog_id, origin_workspace_id)
    WHERE origin = 'workspace_assignment';

CREATE TABLE perm.grant_principals (
    grant_id uuid NOT NULL REFERENCES perm.grants(id) ON DELETE CASCADE,
    kind     perm.principal_kind NOT NULL,
    ref      text NOT NULL,
    mode     perm.principal_mode NOT NULL,
    PRIMARY KEY (grant_id, kind, ref, mode)
);

CREATE TABLE perm.rules (
    id            uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    name          text NOT NULL UNIQUE,
    kind          perm.rule_kind NOT NULL,
    -- mask: the Ranger mask type + optional expression. row_filter: the expression.
    mask_type     text,
    value_expr    text,
    filter_expr   text,
    selector_kind perm.selector_kind NOT NULL,
    -- D6: tags are referenced by KEY. `tags.name` is mutable and non-unique,
    -- and referencing it is what makes a rename break enforcement silently.
    tag_key       text REFERENCES tags(key) ON UPDATE CASCADE,
    catalog_id    uuid REFERENCES registry.catalogs(id) ON DELETE CASCADE,
    schema_name   text,
    table_name    text,
    column_name   text,
    created_at    timestamptz NOT NULL DEFAULT now(),
    CHECK ((selector_kind = 'tag') = (tag_key IS NOT NULL)),
    CHECK ((selector_kind = 'path') = (catalog_id IS NOT NULL)),
    -- A tag selector carries no path at all — not just no catalog_id — so a
    -- tag rule cannot silently carry a schema/table/column that
    -- rule_from_row would then drop on the floor.
    CHECK (selector_kind = 'path' OR
           (schema_name IS NULL AND table_name IS NULL AND column_name IS NULL)),
    -- Same level-skip rule as perm.grants: a column implies a table implies
    -- a schema. (This mirrors perm.grants exactly; egret_perm::model's own
    -- doc comment claims this guard exists on perm.rules too, and until now
    -- it did not.)
    CHECK (schema_name IS NOT NULL OR table_name IS NULL),
    CHECK (table_name IS NOT NULL OR column_name IS NULL),
    CHECK ((kind = 'mask') = (mask_type IS NOT NULL)),
    CHECK ((kind = 'row_filter') = (filter_expr IS NOT NULL)),
    -- value_expr is mask-only payload (an optional value for the mask
    -- expression); a row_filter rule has no use for it.
    CHECK (kind = 'mask' OR value_expr IS NULL)
);

-- Same shape as perm.grant_principals, including the `mode` column: a rule's
-- exemption set is a PrincipalSet (include + exclude) exactly like a grant's
-- principals, so "exempt this whole group except these people" round-trips
-- the same way "grant this whole group except these people" does. The brief's
-- literal SQL omitted `mode` here; that would have silently dropped
-- Rule.exempt.exclude on every write, the same defect class ("a predicate in
-- one code path is a rule somebody later forgets") the CHECK constraints
-- above exist to prevent. Added for symmetry with grant_principals.
CREATE TABLE perm.rule_exemptions (
    rule_id uuid NOT NULL REFERENCES perm.rules(id) ON DELETE CASCADE,
    kind    perm.principal_kind NOT NULL,
    ref     text NOT NULL,
    mode    perm.principal_mode NOT NULL,
    PRIMARY KEY (rule_id, kind, ref, mode)
);

-- The reverse index Ranger has no equivalent of: which egret object produced
-- which policy, in which profile. Read to answer "where is this rule applied",
-- and used by a recompile to DIFF rather than drop-and-recreate, so a policy id
-- an engine has cached does not churn.
CREATE TABLE perm.compiled (
    service_id  bigint NOT NULL REFERENCES policy.authz_services(id) ON DELETE CASCADE,
    -- egret deletes individual policies by id (egret-policy's
    -- PolicyStore::delete_policy), not only whole services. Without this FK
    -- a deleted policy leaves its perm.compiled row behind, and a recompile
    -- diffing against a dangling row concludes the policy still exists and
    -- updates it in place instead of recreating it — the exact
    -- silently-lost-enforcement failure this table exists to prevent.
    policy_id   bigint NOT NULL REFERENCES policy.authz_policies(id) ON DELETE CASCADE,
    source_kind text NOT NULL CHECK (source_kind IN ('grant', 'rule')),
    grant_id    uuid REFERENCES perm.grants(id) ON DELETE CASCADE,
    rule_id     uuid REFERENCES perm.rules(id) ON DELETE CASCADE,
    compiled_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (service_id, policy_id),
    CHECK ((source_kind = 'grant') = (grant_id IS NOT NULL)),
    CHECK ((source_kind = 'rule') = (rule_id IS NOT NULL))
);
CREATE INDEX compiled_by_grant ON perm.compiled (grant_id);
CREATE INDEX compiled_by_rule  ON perm.compiled (rule_id);
