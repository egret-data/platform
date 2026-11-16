-- SP5a (governance full privilege vocabulary, concepts spec D4): exactly one
-- OWNER per securable. A securable is keyed the way `perm.grants` keys one:
-- (catalog_id, schema_name, table_name, column_name), each level NULL when the
-- securable is the level above (all NULL below the catalog = the catalog).
--
-- The owner is read into the IR as an allow of every privilege on the securable
-- and its subtree (see `egret_perm::owners`). It is additive: this migration
-- seeds NOTHING. Backfilling owners for existing securables (catalog creator,
-- else the platform admins group) is SP5b's job — a real owner row adds
-- read/write/create/manage reach and so changes the compiled engine policy.
--
-- Transfer = UPDATE of the holder; the UNIQUE key is the securable, so one
-- securable can never have two owners.
CREATE TABLE perm.securable_owners (
    id             uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    catalog_id     uuid NOT NULL REFERENCES registry.catalogs(id) ON DELETE CASCADE,
    schema_name    text,
    table_name     text,
    column_name    text,
    principal_kind perm.principal_kind NOT NULL,
    principal_ref  text NOT NULL,
    created_at     timestamptz NOT NULL DEFAULT now(),
    updated_at     timestamptz NOT NULL DEFAULT now(),
    CHECK (schema_name IS NOT NULL OR table_name IS NULL),
    CHECK (table_name IS NOT NULL OR column_name IS NULL),
    CONSTRAINT securable_owners_one_per_securable
        UNIQUE NULLS NOT DISTINCT (catalog_id, schema_name, table_name, column_name)
);
